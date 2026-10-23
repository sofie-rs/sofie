use std::sync::Arc;

use sofie_client::Outgoing;
use sofie_core::{Error, Result};
use sofie_db::Db;

use crate::provider::{Checkout, Provider, ProviderKeys, Vendor};
use crate::{Link, Plans, Subscriber};

const BASE_URL: &str = "SOFIE_BILLING_BASE_URL";

/// The provider and prices billing is set up with: `sofie::run` reads them from
/// `SOFIE_BILLING_*`; without a provider plans, trials and quotas still work.
#[derive(Clone)]
pub struct Setup {
    pub(crate) provider: Option<Arc<dyn Provider>>,
    /// Plan or product name, and the provider's price id for it.
    pub(crate) prices: Arc<[(String, String)]>,
    pub(crate) base_url: Option<String>,
}

impl Setup {
    #[must_use]
    pub fn without_provider() -> Self {
        Self {
            provider: None,
            prices: Arc::new([]),
            base_url: None,
        }
    }

    pub fn with_provider(
        vendor: Vendor,
        http: Outgoing,
        keys: ProviderKeys,
        prices: &[(&str, &str)],
        base_url: &str,
    ) -> Result<Self> {
        Ok(Self {
            provider: Some(Arc::from(vendor.build(http, keys))),
            prices: prices
                .iter()
                .map(|(name, price)| ((*name).to_owned(), (*price).to_owned()))
                .collect(),
            base_url: Some(checked_base_url(base_url)?),
        })
    }

    /// `PROVIDER` (`stripe` or `paddle`) and with it `API_KEY`, `WEBHOOK_SECRET`,
    /// `PRICES` (`pro=price_123,credits=price_456`) and `API_URL`; `BASE_URL`, else `SOFIE_BASE_URL`.
    #[doc(hidden)]
    pub fn from_environment() -> Result<Self> {
        let Some(vendor) = variable("PROVIDER")? else {
            return Ok(Self::without_provider());
        };
        let vendor = Vendor::parse(&vendor)?;
        let required = |name: &str| {
            variable(name)?.ok_or_else(|| {
                Error::internal_message(format!(
                    "SOFIE_BILLING_{name} is not set; billing through {} needs it",
                    vendor.name()
                ))
            })
        };
        let keys = ProviderKeys {
            api_key: required("API_KEY")?,
            webhook_secret: required("WEBHOOK_SECRET")?,
        };
        let base_url = provider_base_url(sofie_core::environment_variable, vendor.name())?;
        let prices = parse_prices(&variable("PRICES")?.unwrap_or_default())?;
        let api_url = variable("API_URL")?.unwrap_or_else(|| vendor.api_url().to_owned());
        let settings = sofie_client::Vendor {
            base_url: Some(api_url),
            ..sofie_client::Vendor::default()
        };
        let http = sofie_client::for_vendor(vendor.name(), settings)?;
        let pairs: Vec<(&str, &str)> = prices
            .iter()
            .map(|(name, price)| (name.as_str(), price.as_str()))
            .collect();
        Self::with_provider(vendor, http, keys, &pairs, &base_url)
    }

    pub(crate) fn check_prices(&self, plans: &Plans) -> Result<()> {
        let unknown = self
            .prices
            .iter()
            .find(|(name, _)| plans.plan(name).is_none() && plans.product(name).is_none());
        match unknown {
            Some((name, _)) => Err(Error::internal_message(format!(
                "SOFIE_BILLING_PRICES prices `{name}`, which is neither a plan nor a product"
            ))),
            None => Ok(()),
        }
    }

    fn price_of(&self, name: &str) -> Result<&str> {
        self.prices
            .iter()
            .find(|(priced, _)| priced == name)
            .map(|(_, price)| price.as_str())
            .ok_or_else(|| {
                Error::internal_message(format!(
                    "`{name}` has no price; add `{name}=<price id>` to SOFIE_BILLING_PRICES"
                ))
            })
    }

    pub(crate) fn name_of(&self, price: &str) -> Result<&str> {
        self.prices
            .iter()
            .find(|(_, priced)| priced == price)
            .map(|(name, _)| name.as_str())
            .ok_or_else(|| {
                Error::internal_message(format!(
                    "the provider reported price `{price}`, which SOFIE_BILLING_PRICES does not name"
                ))
            })
    }

    pub(crate) async fn checkout(
        &self,
        subscriber: Subscriber,
        provider_customer: Option<&str>,
        name: &str,
        quantity: u32,
        recurring: bool,
        return_path: &str,
    ) -> Result<Link> {
        if quantity == 0 {
            return Err(Error::invalid("quantity", "must be at least 1"));
        }
        let return_url = self.return_url(return_path)?;
        let checkout = Checkout {
            subscriber,
            price: self.price_of(name)?,
            quantity,
            recurring,
            provider_customer,
            return_url: &return_url,
        };
        let location = self.provider()?.checkout(&checkout).await?;
        Ok(Link { location })
    }

    pub(crate) async fn portal(&self, provider_customer: &str, return_path: &str) -> Result<Link> {
        let return_url = self.return_url(return_path)?;
        let location = self
            .provider()?
            .portal(provider_customer, &return_url)
            .await?;
        Ok(Link { location })
    }

    pub(crate) fn provider(&self) -> Result<&dyn Provider> {
        self.provider.as_deref().ok_or_else(|| {
            Error::internal_message(
                "billing has no payment provider; set SOFIE_BILLING_PROVIDER and its keys",
            )
        })
    }

    fn return_url(&self, path: &str) -> Result<String> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(Error::internal_message(format!(
                "a billing return path names a path of this app, not `{path}`"
            )));
        }
        let base = self
            .base_url
            .as_deref()
            .ok_or_else(|| sofie_core::base_url_missing(BASE_URL, "billing returns to the app"))?;
        Ok(format!("{base}{path}"))
    }
}

/// Every handler, job and task on this database reaches billing through it.
#[doc(hidden)]
pub fn configure(db: &Db, setup: Setup) {
    sofie_db::provide(db, Arc::new(setup));
}

fn variable(name: &str) -> Result<Option<String>> {
    let key = format!("SOFIE_BILLING_{name}");
    Ok(sofie_core::environment_variable(&key)?.filter(|value| !value.trim().is_empty()))
}

fn provider_base_url(
    lookup: impl Fn(&str) -> Result<Option<String>>,
    vendor: &str,
) -> Result<String> {
    let variable = sofie_core::base_url(BASE_URL, lookup)?.ok_or_else(|| {
        sofie_core::base_url_missing(BASE_URL, &format!("billing through {vendor} needs it"))
    })?;
    Ok(variable.value)
}

fn parse_prices(text: &str) -> Result<Vec<(String, String)>> {
    text.split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, price) = pair.split_once('=').ok_or_else(|| {
                Error::internal_message(format!(
                    "SOFIE_BILLING_PRICES holds `{pair}`, not `<plan or product>=<price id>`"
                ))
            })?;
            Ok((name.trim().to_owned(), price.trim().to_owned()))
        })
        .collect()
}

fn checked_base_url(url: &str) -> Result<String> {
    let url = url.trim_end_matches('/');
    let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
    let clean =
        !url.contains(|character: char| character.is_whitespace() || "\"'<>?#".contains(character));
    if scheme_ok && clean {
        return Ok(url.to_owned());
    }
    Err(Error::internal_message(format!(
        "the billing base URL `{url}` ({BASE_URL} or {}) is not an http or https URL without a query",
        sofie_core::BASE_URL
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_pair_names_with_price_ids() {
        assert_eq!(
            parse_prices(" pro=price_1 , credits=price_2,").unwrap(),
            [
                ("pro".to_owned(), "price_1".to_owned()),
                ("credits".to_owned(), "price_2".to_owned())
            ]
        );
        assert!(parse_prices("pro").is_err());
    }

    #[test]
    fn the_shared_base_url_serves_unless_billing_has_its_own() {
        fn given<'a>(
            variables: &'a [(&str, &str)],
        ) -> impl Fn(&str) -> Result<Option<String>> + 'a {
            |name| {
                Ok(variables
                    .iter()
                    .find(|(variable, _)| *variable == name)
                    .map(|(_, value)| (*value).to_owned()))
            }
        }
        let shared = (sofie_core::BASE_URL, "https://app.test");
        assert_eq!(
            provider_base_url(given(&[shared]), "stripe").unwrap(),
            "https://app.test"
        );
        let own = (BASE_URL, "https://billing.app.test");
        assert_eq!(
            provider_base_url(given(&[shared, own]), "stripe").unwrap(),
            "https://billing.app.test"
        );
        let missing = provider_base_url(given(&[]), "stripe").unwrap_err();
        assert!(
            missing
                .to_string()
                .starts_with("neither SOFIE_BILLING_BASE_URL nor SOFIE_BASE_URL is set"),
            "{missing}"
        );
    }

    #[test]
    fn a_base_url_is_http_without_a_query_or_trailing_slash() {
        assert_eq!(
            checked_base_url("https://app.test/").unwrap(),
            "https://app.test"
        );
        assert!(checked_base_url("javascript:alert(1)").is_err());
        assert!(checked_base_url("https://a.test/\"x").is_err());
        assert!(checked_base_url("https://app.test/?campaign=x").is_err());
        assert!(checked_base_url("https://app.test/#top").is_err());
    }
}
