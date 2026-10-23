use serde_json::{Value, json};
use sofie_core::{Error, Fnv1a64, Result, Uuid};

/// A request without a principal, and a job, are in only at 100 percent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub on: bool,
    pub environments: Vec<String>,
    pub areas: Vec<String>,
    pub principals: Vec<Uuid>,
    pub percentage: u8,
    pub variants: Vec<(String, u32)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Subject {
    pub area: Option<&'static str>,
    pub principal: Option<Uuid>,
}

impl Rule {
    #[must_use]
    pub fn off() -> Self {
        Self {
            on: false,
            environments: Vec::new(),
            areas: Vec::new(),
            principals: Vec::new(),
            percentage: 100,
            variants: Vec::new(),
        }
    }

    #[must_use]
    pub fn everyone() -> Self {
        Self {
            on: true,
            ..Self::off()
        }
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        let one_variant = self
            .variants
            .iter()
            .filter(|(_, weight)| *weight > 0)
            .count()
            <= 1;
        !self.on
            || (self.environments.is_empty()
                && self.areas.is_empty()
                && self.percentage == 100
                && one_variant)
    }

    pub(crate) fn is_on(&self, name: &str, environment: &str, subject: Subject) -> bool {
        let listed =
            |list: &[String], value: &str| list.is_empty() || list.iter().any(|item| item == value);
        if !self.on || !listed(&self.environments, environment) {
            return false;
        }
        if !self.areas.is_empty() && !subject.area.is_some_and(|area| listed(&self.areas, area)) {
            return false;
        }
        if self.percentage >= 100 {
            return true;
        }
        subject.principal.is_some_and(|principal| {
            self.principals.contains(&principal)
                || bucket(&[name], principal, 100) < u64::from(self.percentage)
        })
    }

    pub(crate) fn variant(&self, name: &str, subject: Subject) -> Option<&str> {
        let weighted = self.variants.iter().filter(|(_, weight)| *weight > 0);
        let total: u64 = weighted.clone().map(|(_, weight)| u64::from(*weight)).sum();
        let Some(principal) = subject.principal else {
            return weighted.map(|(variant, _)| variant.as_str()).next();
        };
        let mut point = bucket(&[name, "/variant"], principal, total.max(1));
        for (variant, weight) in weighted {
            match point.checked_sub(u64::from(*weight)) {
                Some(rest) => point = rest,
                None => return Some(variant),
            }
        }
        None
    }

    pub(crate) fn to_json(&self) -> String {
        let principals: Vec<String> = self.principals.iter().map(ToString::to_string).collect();
        json!({
            "on": self.on,
            "environments": self.environments,
            "areas": self.areas,
            "principals": principals,
            "percentage": self.percentage,
            "variants": self.variants,
        })
        .to_string()
    }

    pub(crate) fn from_json(name: &str, text: &str) -> Result<Self> {
        let malformed = |what: &str| {
            Error::internal_message(format!("the stored rule of flag `{name}` has {what}"))
        };
        let value: Value = serde_json::from_str(text).map_err(|error| {
            Error::internal(format!("reading the stored rule of flag `{name}`"), error)
        })?;
        let texts = |key: &str| -> Result<Vec<String>> {
            value[key]
                .as_array()
                .ok_or_else(|| malformed(&format!("no list `{key}`")))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| malformed(&format!("a non-text entry in `{key}`")))
                })
                .collect()
        };
        let principals = texts("principals")?
            .iter()
            .map(|text| {
                text.parse()
                    .map_err(|_| malformed("a principal that is not an id"))
            })
            .collect::<Result<_>>()?;
        let rule = Self {
            on: value["on"]
                .as_bool()
                .ok_or_else(|| malformed("no switch `on`"))?,
            environments: texts("environments")?,
            areas: texts("areas")?,
            principals,
            percentage: value["percentage"]
                .as_u64()
                .and_then(|percentage| u8::try_from(percentage).ok())
                .ok_or_else(|| malformed("no `percentage`"))?,
            variants: variants_of(&value["variants"])
                .ok_or_else(|| malformed("malformed `variants`"))?,
        };
        rule.validate(name)?;
        Ok(rule)
    }

    pub fn validate(&self, name: &str) -> Result<()> {
        if self.percentage > 100 {
            return Err(Error::invalid(
                "percentage",
                format!("of flag `{name}` must be between 0 and 100"),
            ));
        }
        refuse_malformed_entries("environments", &self.environments)?;
        refuse_malformed_entries("areas", &self.areas)?;
        let variants: Vec<String> = self
            .variants
            .iter()
            .map(|(variant, _)| variant.clone())
            .collect();
        refuse_malformed_entries("variants", &variants)
    }
}

/// An entry is matched against a name the code declares, so one that could never match is refused.
fn refuse_malformed_entries(field: &str, entries: &[String]) -> Result<()> {
    let named = |entry: &String| {
        !entry.is_empty()
            && entry
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
    };
    if let Some(malformed) = entries.iter().find(|entry| !named(entry)) {
        return Err(Error::invalid(
            field,
            format!("`{malformed}` is not a name of letters, digits, `_` and `-`"),
        ));
    }
    let repeated = entries
        .iter()
        .enumerate()
        .find(|(index, entry)| entries[..*index].contains(entry));
    if let Some((_, entry)) = repeated {
        return Err(Error::invalid(field, format!("lists `{entry}` twice")));
    }
    Ok(())
}

fn variants_of(value: &Value) -> Option<Vec<(String, u32)>> {
    value
        .as_array()?
        .iter()
        .map(|pair| {
            let name = pair.get(0)?.as_str()?.to_owned();
            let weight = u32::try_from(pair.get(1)?.as_u64()?).ok()?;
            Some((name, weight))
        })
        .collect()
}

/// The parts hash as their concatenation, so splitting a name never moves anyone's bucket.
fn bucket(name: &[&str], principal: Uuid, range: u64) -> u64 {
    let hash = name
        .iter()
        .fold(Fnv1a64::new(), |hash, part| hash.write(part.as_bytes()))
        .write(&[0])
        .write(principal.as_bytes())
        .finish();
    mixed(hash) % range
}

/// FNV-1a leaves its low bits a function of the input bytes' parity, so two flags' buckets
/// would decide each other; splitmix64's finalizer spreads every input bit over the output.
fn mixed(mut hash: u64) -> u64 {
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn someone(n: u128) -> Subject {
        Subject {
            area: Some("member"),
            principal: Some(principal(n)),
        }
    }

    #[test]
    fn off_is_off_and_everyone_is_on_for_anyone() {
        assert!(!Rule::off().is_on("a.b", "production", someone(1)));
        assert!(Rule::everyone().is_on("a.b", "production", Subject::default()));
    }

    #[test]
    fn environments_and_areas_narrow_it() {
        let rule = Rule {
            environments: vec!["staging".to_owned()],
            areas: vec!["console".to_owned()],
            ..Rule::everyone()
        };
        let console = Subject {
            area: Some("console"),
            principal: None,
        };
        assert!(rule.is_on("a.b", "staging", console));
        assert!(!rule.is_on("a.b", "production", console));
        assert!(!rule.is_on("a.b", "staging", someone(1)));
        assert!(!rule.is_on("a.b", "staging", Subject::default()));
    }

    fn split() -> Rule {
        Rule {
            variants: vec![("a".to_owned(), 1), ("b".to_owned(), 1)],
            ..Rule::everyone()
        }
    }

    #[test]
    fn two_flags_split_principals_independently() {
        let (first, second) = (split(), split());
        let principals =
            (1..=4_000_u128).map(|n| n.wrapping_mul(0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c835));
        let same = principals
            .filter(|n| {
                let subject = someone(*n);
                first.variant("a.first", subject) == second.variant("a.second", subject)
            })
            .count();
        assert!((1_700..2_300).contains(&same), "{same}");
    }

    #[test]
    fn a_rollout_does_not_choose_its_principals_variant() {
        let rule = Rule {
            percentage: 10,
            ..split()
        };
        let rolled_in: Vec<Subject> = (1..=20_000_u128)
            .map(someone)
            .filter(|subject| rule.is_on("a.b", "x", *subject))
            .collect();
        let first_arm = rolled_in
            .iter()
            .filter(|subject| rule.variant("a.b", **subject) == Some("a"))
            .count();
        let share = first_arm * 100 / rolled_in.len();
        assert!(
            (40..=60).contains(&share),
            "{share}% of {}",
            rolled_in.len()
        );
    }

    #[test]
    fn a_percentage_is_stable_per_principal_and_near_its_share() {
        let rule = Rule {
            percentage: 30,
            ..Rule::everyone()
        };
        let on = (0..10_000)
            .filter(|n| rule.is_on("a.b", "x", someone(*n)))
            .count();
        assert!((2_700..3_300).contains(&on), "{on}");
        let first: Vec<bool> = (0..50)
            .map(|n| rule.is_on("a.b", "x", someone(n)))
            .collect();
        let again: Vec<bool> = (0..50)
            .map(|n| rule.is_on("a.b", "x", someone(n)))
            .collect();
        assert_eq!(first, again);
        assert!(!rule.is_on("a.b", "x", Subject::default()));
    }

    #[test]
    fn listed_principals_are_in_whatever_the_percentage() {
        let rule = Rule {
            percentage: 0,
            principals: vec![principal(7)],
            ..Rule::everyone()
        };
        assert!(rule.is_on("a.b", "x", someone(7)));
        assert!(!rule.is_on("a.b", "x", someone(8)));
    }

    #[test]
    fn variants_split_by_weight_and_default_to_the_first() {
        let rule = Rule {
            variants: vec![("classic".to_owned(), 1), ("compact".to_owned(), 3)],
            ..Rule::everyone()
        };
        let compact = (0..4_000)
            .filter(|n| rule.variant("a.b", someone(*n)) == Some("compact"))
            .count();
        assert!((2_800..3_200).contains(&compact), "{compact}");
        assert_eq!(rule.variant("a.b", Subject::default()), Some("classic"));
    }

    #[test]
    fn a_rule_survives_its_stored_form() {
        let rule = Rule {
            environments: vec!["production".to_owned()],
            areas: vec!["member".to_owned()],
            principals: vec![principal(3)],
            percentage: 25,
            variants: vec![("a".to_owned(), 1)],
            ..Rule::everyone()
        };
        assert_eq!(Rule::from_json("a.b", &rule.to_json()).unwrap(), rule);
        assert!(Rule::from_json("a.b", "{\"on\":true}").is_err());
    }

    #[test]
    fn a_target_that_could_never_match_is_refused() {
        for refused in [
            Rule {
                areas: vec![String::new()],
                ..Rule::everyone()
            },
            Rule {
                environments: vec!["pro duction".to_owned()],
                ..Rule::everyone()
            },
            Rule {
                variants: vec![("a".to_owned(), 1), ("a".to_owned(), 2)],
                ..Rule::everyone()
            },
        ] {
            assert!(refused.validate("a.b").is_err(), "{refused:?}");
        }
    }

    #[test]
    fn settled_means_the_same_for_everyone() {
        assert!(Rule::off().is_settled());
        assert!(Rule::everyone().is_settled());
        let partial = Rule {
            percentage: 50,
            ..Rule::everyone()
        };
        assert!(!partial.is_settled());
    }
}
