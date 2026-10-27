mod encoding;

use sofie_core::{Error, Result};

use crate::Currency;
use crate::decimal::{Refusal, divide_rounded, parse_scaled, scaled_text, write_scaled};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Money {
    minor: i64,
    currency: Currency,
}

impl Money {
    #[must_use]
    pub fn new(minor: i64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    #[must_use]
    pub fn zero(currency: Currency) -> Self {
        Self::new(0, currency)
    }

    pub fn parse_major(text: &str, currency: Currency) -> Result<Money> {
        let digits = currency.minor_digits();
        let minor = parse_scaled(text, digits).map_err(|refusal| {
            let message = match refusal {
                Refusal::TooManyDecimals if digits == 0 => {
                    format!("{currency} amounts have no decimals")
                }
                Refusal::TooManyDecimals => {
                    format!("{currency} amounts have at most {digits} decimals")
                }
                other => other.message().to_owned(),
            };
            Error::invalid("amount", message)
        })?;
        Ok(Self::new(minor, currency))
    }

    #[must_use]
    pub fn format_major(&self) -> String {
        scaled_text(self.minor, self.currency.minor_digits())
    }

    pub fn write_major(&self, out: &mut String) {
        write_scaled(out, self.minor, self.currency.minor_digits());
    }

    #[must_use]
    pub fn minor(&self) -> i64 {
        self.minor
    }

    #[must_use]
    pub fn currency(&self) -> Currency {
        self.currency
    }

    #[must_use]
    pub fn is_positive(&self) -> bool {
        self.minor > 0
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.minor == 0
    }

    pub fn checked_add(self, other: Money) -> Result<Money> {
        self.combine(other, "add", i64::checked_add)
    }

    pub fn checked_subtract(self, other: Money) -> Result<Money> {
        self.combine(other, "subtract", i64::checked_sub)
    }

    /// The product is exact before it is rounded once, half away from zero.
    pub fn scale(self, numerator: i64, denominator: i64) -> Result<Money> {
        if denominator <= 0 {
            return Err(Error::internal_message(format!(
                "cannot scale an amount by {numerator}/{denominator}: the denominator must be \
                 positive"
            )));
        }
        let product = i128::from(self.minor) * i128::from(numerator);
        let minor = divide_rounded(product, i128::from(denominator)).ok_or_else(too_large)?;
        Ok(Self::new(minor, self.currency))
    }

    fn combine(
        self,
        other: Money,
        operation: &str,
        apply: fn(i64, i64) -> Option<i64>,
    ) -> Result<Money> {
        // Amounts reach arithmetic already in one currency, so a mix is the
        // caller's bug, not something the person asking can correct.
        if self.currency != other.currency {
            return Err(Error::internal_message(format!(
                "cannot {operation} {} and {} amounts",
                self.currency, other.currency
            )));
        }
        let minor = apply(self.minor, other.minor).ok_or_else(too_large)?;
        Ok(Self::new(minor, self.currency))
    }
}

fn too_large() -> Error {
    Error::Rule("The amount is too large.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::euros;

    fn dollars(minor: i64) -> Money {
        Money::new(minor, Currency::parse("USD").unwrap())
    }

    #[test]
    fn adds_and_subtracts_in_the_same_currency() {
        assert_eq!(euros(1000).checked_add(euros(250)).unwrap(), euros(1250));
        assert_eq!(
            euros(1000).checked_subtract(euros(1250)).unwrap(),
            euros(-250)
        );
    }

    #[test]
    fn refuses_arithmetic_across_currencies_as_a_bug() {
        assert!(matches!(
            euros(1).checked_add(dollars(1)),
            Err(Error::Internal { .. })
        ));
        assert!(matches!(
            euros(1).checked_subtract(dollars(1)),
            Err(Error::Internal { .. })
        ));
    }

    #[test]
    fn refuses_overflow_without_showing_minor_units() {
        assert_eq!(
            format!("{:#}", euros(i64::MAX).checked_add(euros(1)).unwrap_err()),
            "The amount is too large."
        );
        assert!(matches!(
            euros(i64::MIN).checked_subtract(euros(1)),
            Err(Error::Rule(_))
        ));
    }

    #[test]
    fn reports_its_sign() {
        let zero = Money::zero(Currency::parse("EUR").unwrap());

        assert!(zero.is_zero() && !zero.is_positive());
        assert!(euros(1).is_positive() && !euros(1).is_zero());
        assert!(!euros(-1).is_positive());
    }

    #[test]
    fn parses_and_formats_major_units_by_its_currency() {
        let yen = Currency::parse("JPY").unwrap();

        assert_eq!(
            Money::parse_major("149.9", euros(0).currency()).unwrap(),
            euros(14990)
        );
        assert_eq!(
            Money::parse_major("1500", yen).unwrap(),
            Money::new(1500, yen)
        );
        assert!(matches!(
            Money::parse_major("1.5", yen),
            Err(Error::Invalid(_))
        ));
        assert_eq!(euros(14990).format_major(), "149.90");
        assert_eq!(Money::new(-1500, yen).format_major(), "-1500");
    }

    #[test]
    fn parses_by_the_currency_minor_unit() {
        let refusal = |text: &str, code: &str| {
            Money::parse_major(text, Currency::parse(code).unwrap())
                .unwrap_err()
                .to_string()
        };

        assert_eq!(
            refusal("1500.0", "JPY"),
            "invalid input: amount: JPY amounts have no decimals;"
        );
        assert_eq!(
            refusal("1.0005", "KWD"),
            "invalid input: amount: KWD amounts have at most 3 decimals;"
        );
        assert_eq!(
            refusal("1,5", "EUR"),
            "invalid input: amount: must use one `.` before the decimals and no other separator;"
        );
    }

    #[test]
    fn scales_by_a_ratio_exactly_before_rounding_once() {
        assert_eq!(euros(1000).scale(1, 3).unwrap(), euros(333));
        assert_eq!(euros(1000).scale(2, 3).unwrap(), euros(667));
        assert_eq!(euros(i64::MAX).scale(3, 3).unwrap(), euros(i64::MAX));
        assert!(matches!(euros(i64::MAX).scale(2, 1), Err(Error::Rule(_))));
        assert!(matches!(euros(1).scale(1, 0), Err(Error::Internal { .. })));
        assert!(matches!(euros(1).scale(1, -2), Err(Error::Internal { .. })));
    }
}
