//! Addresses mail no longer goes to, kept in sofie-postgres's `migrations/0012_mail_suppressions.sql`.

use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, SqlType, SqlValue, Uuid};
use sofie_db::{Aggregate, Behavior, Column, Field, Meta, Read, Select, Timestamps};

use crate::{Address, Mail};

const TEXT: &[ColumnSpec] = &[ColumnSpec::new("", SqlType::Text)];
const LIST_PREFIX: &str = "list:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Nothing: the address bounced for good.
    All,
    /// No bulk mail of any list: its owner complained of one.
    Bulk,
    /// No bulk mail of this list: its owner unsubscribed.
    List(String),
}

impl Scope {
    fn to_stored(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Bulk => "bulk".to_owned(),
            Self::List(list) => format!("{LIST_PREFIX}{list}"),
        }
    }

    fn from_stored(text: &str) -> Result<Self> {
        match text {
            "all" => Ok(Self::All),
            "bulk" => Ok(Self::Bulk),
            _ => text
                .strip_prefix(LIST_PREFIX)
                .map(|list| Self::List(list.to_owned()))
                .ok_or_else(|| {
                    Error::internal_message("a stored suppression has an unknown scope")
                }),
        }
    }

    /// Transactional mail has no list and stops only for `All`.
    pub(crate) fn blocks(&self, list: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Bulk => list.is_some(),
            Self::List(suppressed) => list == Some(suppressed.as_str()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Bounce,
    Complaint,
    Unsubscribe,
}

impl Reason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bounce => "bounce",
            Self::Complaint => "complaint",
            Self::Unsubscribe => "unsubscribe",
        }
    }

    fn parse(text: &str) -> Result<Self> {
        match text {
            "bounce" => Ok(Self::Bounce),
            "complaint" => Ok(Self::Complaint),
            "unsubscribe" => Ok(Self::Unsubscribe),
            _ => Err(Error::internal_message(
                "a stored suppression has an unknown reason",
            )),
        }
    }
}

pub struct Suppression {
    meta: Meta,
    address: String,
    scope: Scope,
    reason: Reason,
}

const ADDRESS: Column<Suppression, String> = Column::new("address");

impl Aggregate for Suppression {
    const NAME: &'static str = "mail_suppressions";
    const MODULE: &'static str = "sofie";
    const FIELDS: &'static [Field] = &[
        Field::new("address", String::COLUMNS, false),
        Field::new("scope", TEXT, false),
        Field::new("reason", TEXT, false),
    ];
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[&Timestamps];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.address.encode(out);
        out.push(SqlValue::Text(self.scope.to_stored()));
        out.push(SqlValue::Text(self.reason.as_str().to_owned()));
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            address: String::decode(row)?,
            scope: Scope::from_stored(row.text()?)?,
            reason: Reason::parse(row.text()?)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        None
    }
}

impl Suppression {
    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    #[must_use]
    pub fn reason(&self) -> Reason {
        self.reason
    }
}

fn normalized(address: &str) -> String {
    address.trim().to_lowercase()
}

pub async fn scopes(read: &Read, address: &str) -> Result<Vec<Scope>> {
    let held = Suppression::query()
        .where_(ADDRESS.eq(normalized(address)))
        .all(read)
        .await?;
    Ok(held
        .into_iter()
        .map(|suppression| suppression.scope)
        .collect())
}

pub(crate) async fn reachable(read: &Read, mail: &Mail) -> Result<Vec<Address>> {
    let emails = mail.to().iter().map(|to| normalized(to.email().as_str()));
    let held = Suppression::query()
        .where_(ADDRESS.in_(emails))
        .all(read)
        .await?;
    let reachable = mail.to().iter().filter(|to| {
        let email = normalized(to.email().as_str());
        !held.iter().any(|suppression| {
            suppression.address == email && suppression.scope.blocks(mail.list())
        })
    });
    Ok(reachable.cloned().collect())
}

/// A new suppression, or none when one already covers the scope.
pub async fn suppress(
    read: &Read,
    address: &str,
    scope: Scope,
    reason: Reason,
) -> Result<Option<Suppression>> {
    Ok(suppress_each(read, &[address], &scope, reason).await?.pop())
}

/// A new suppression for each address no held one covers, read in one query.
pub(crate) async fn suppress_each(
    read: &Read,
    addresses: &[&str],
    scope: &Scope,
    reason: Reason,
) -> Result<Vec<Suppression>> {
    let mut addresses: Vec<String> = addresses
        .iter()
        .map(|address| normalized(address))
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Ok(Vec::new());
    }
    let held = Suppression::query()
        .where_(ADDRESS.in_(addresses.iter().cloned()))
        .all(read)
        .await?;
    let covered = |address: &str| {
        held.iter().any(|suppression| {
            suppression.address == address
                && (suppression.scope == *scope || suppression.scope == Scope::All)
        })
    };
    addresses
        .into_iter()
        .filter(|address| !covered(address))
        .map(|address| {
            Ok(Suppression {
                meta: Meta::new()?,
                address,
                scope: scope.clone(),
                reason,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_blocks_only_the_mail_it_names() {
        let list = Scope::List("news".to_owned());

        assert!(Scope::All.blocks(None));
        assert!(!Scope::Bulk.blocks(None));
        assert!(Scope::Bulk.blocks(Some("news")));
        assert!(list.blocks(Some("news")));
        assert!(!list.blocks(Some("offers")));
        assert!(!list.blocks(None));
        assert_eq!(Scope::from_stored(&list.to_stored()).unwrap(), list);
    }

    #[test]
    fn a_suppressed_address_is_written_as_a_parsed_address_is() {
        let parsed = sofie_values::Email::parse("ÇAĞRI@Example.com").unwrap();

        assert_eq!(normalized(" ÇAĞRI@Example.com "), parsed.as_str());
    }
}
