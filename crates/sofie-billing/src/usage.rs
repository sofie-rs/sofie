use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span};
use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue, Timestamp, Uuid};
use sofie_db::{Aggregate, Column, Dialect, Field, Meta, Outcome, Plan, Read, Select, Step};

use crate::{Period, Quota, Subscriber};

pub(crate) struct Usage {
    meta: Meta,
    subscriber: Uuid,
    quota: String,
    period_start: Timestamp,
    used: i64,
}

const SUBSCRIBER: Column<Usage, Uuid> = Column::new("subscriber");
const QUOTA: Column<Usage, String> = Column::new("quota");
const PERIOD_START: Column<Usage, Timestamp> = Column::new("period_start");

pub(crate) async fn used(
    read: &Read,
    subscriber: Subscriber,
    quota: Quota,
    period_start: Timestamp,
) -> Result<u64> {
    let usage = Usage::query()
        .where_(SUBSCRIBER.eq(subscriber.uuid()))
        .where_(QUOTA.eq(quota.name().to_owned()))
        .where_(PERIOD_START.eq(period_start))
        .first(read)
        .await?;
    Ok(usage.map_or(0, |usage| u64::try_from(usage.used).unwrap_or(0)))
}

/// Committed with the handler's outcome. A use that would pass the allowance,
/// counted by another request since this one read, rolls back as a conflict.
#[must_use = "the use is counted only when the handler returns it"]
pub struct Consumed {
    subscriber: Uuid,
    quota: &'static str,
    period_start: Timestamp,
    amount: i64,
    allowance: i64,
}

impl Consumed {
    pub(crate) fn new(
        subscriber: Uuid,
        quota: &'static str,
        period_start: Timestamp,
        amount: u64,
        allowance: u64,
    ) -> Self {
        Self {
            subscriber,
            quota,
            period_start,
            amount: i64::try_from(amount).unwrap_or(i64::MAX),
            allowance: i64::try_from(allowance).unwrap_or(i64::MAX),
        }
    }
}

/// Aliased, so the upsert names the row it conflicts with the same under either database.
fn consume_sql(dialect: &dyn Dialect) -> String {
    let at = |index| dialect.placeholder(index);
    format!(
        "INSERT INTO {table} AS counted \
         (id, version, subscriber, quota, period_start, used) \
         VALUES ({id}, 1, {subscriber}, {quota}, {period_start}, {amount}) \
         ON CONFLICT (subscriber, quota, period_start) DO UPDATE \
         SET used = counted.used + EXCLUDED.used, version = counted.version + 1 \
         WHERE counted.used + EXCLUDED.used <= {allowance}",
        table = dialect.qualified(Usage::MODULE, Usage::NAME),
        id = at(1),
        subscriber = at(2),
        quota = at(3),
        period_start = at(4),
        amount = at(5),
        allowance = at(6),
    )
}

impl Outcome for Consumed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.push(Step::statement(
            consume_sql(plan.dialect()),
            vec![
                SqlValue::Uuid(Uuid::now_v7()?),
                SqlValue::Uuid(self.subscriber),
                SqlValue::Text(self.quota.to_owned()),
                SqlValue::Timestamp(self.period_start),
                SqlValue::Int(self.amount),
                SqlValue::Int(self.allowance),
            ],
            Some(1),
        ));
        plan.touched::<Usage>(None);
        Ok(())
    }
}

/// The start of the period holding `now`, counted in whole periods from `anchor`, in UTC.
pub(crate) fn period_start(anchor: Timestamp, period: Period, now: Timestamp) -> Result<Timestamp> {
    match period {
        Period::Day => Ok(fixed(anchor, SignedDuration::from_hours(24), now)),
        Period::Week => Ok(fixed(anchor, SignedDuration::from_hours(24 * 7), now)),
        Period::Month => calendar(anchor, 1, now),
        Period::Year => calendar(anchor, 12, now),
    }
}

fn fixed(anchor: Timestamp, length: SignedDuration, now: Timestamp) -> Timestamp {
    let elapsed = now.as_second() - anchor.as_second();
    let periods = elapsed.div_euclid(length.as_secs());
    anchor + SignedDuration::from_secs(periods * length.as_secs())
}

/// Added to the anchor rather than chained, so a period anchored on the 31st ends each month's last day.
fn calendar(anchor: Timestamp, months: i32, now: Timestamp) -> Result<Timestamp> {
    let failed = |error| Error::internal("computing a quota's period", error);
    let start = anchor.to_zoned(TimeZone::UTC).datetime();
    let current = now.to_zoned(TimeZone::UTC).datetime();
    let apart = (i32::from(current.year()) - i32::from(start.year())) * 12
        + i32::from(current.month())
        - i32::from(start.month());
    let mut periods = apart.div_euclid(months);
    loop {
        let candidate = start
            .checked_add(Span::new().months(i64::from(periods * months)))
            .map_err(failed)?;
        if candidate <= current {
            return candidate
                .to_zoned(TimeZone::UTC)
                .map(|zoned| zoned.timestamp())
                .map_err(failed);
        }
        periods -= 1;
    }
}

impl Aggregate for Usage {
    const NAME: &'static str = "usage";
    const MODULE: &'static str = "sofie_billing";
    const FIELDS: &'static [Field] = &[
        Field::new("subscriber", Uuid::COLUMNS, false),
        Field::new("quota", String::COLUMNS, false),
        Field::new("period_start", Timestamp::COLUMNS, false),
        Field::new("used", i64::COLUMNS, false),
    ];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.subscriber.encode(out);
        self.quota.encode(out);
        self.period_start.encode(out);
        self.used.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            subscriber: Uuid::decode(row)?,
            quota: String::decode(row)?,
            period_start: Timestamp::decode(row)?,
            used: i64::decode(row)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn a_month_runs_from_the_anchors_day_and_clamps_to_short_months() {
        let anchor = at("2026-01-31T10:00:00Z");
        let start = |now| period_start(anchor, Period::Month, at(now)).unwrap();

        assert_eq!(start("2026-02-15T00:00:00Z"), at("2026-01-31T10:00:00Z"));
        assert_eq!(start("2026-02-28T10:00:00Z"), at("2026-02-28T10:00:00Z"));
        assert_eq!(start("2026-03-30T00:00:00Z"), at("2026-02-28T10:00:00Z"));
        assert_eq!(start("2026-03-31T11:00:00Z"), at("2026-03-31T10:00:00Z"));
    }

    #[tokio::test]
    async fn uses_add_up_to_the_allowance_and_no_further_on_sqlite() {
        let db = sofie_db::Db::new(sofie_sqlite::Sqlite::memory().unwrap());
        db.schema().migrate(&[crate::migrations()]).await.unwrap();
        let subscriber = Uuid::now_v7().unwrap();
        let start = at("2026-01-01T00:00:00Z");
        let consumed = |amount| Consumed::new(subscriber, "api.calls", start, amount, 5);

        db.commit(&mut consumed(3)).await.unwrap();
        db.commit(&mut consumed(2)).await.unwrap();
        let over = db.commit(&mut consumed(1)).await;

        assert!(matches!(over, Err(Error::Conflict)), "{over:?}");
        let counted = Usage::query().all(&db.read()).await.unwrap();
        assert_eq!(
            counted.iter().map(|usage| usage.used).collect::<Vec<_>>(),
            [5]
        );
    }

    #[test]
    fn days_weeks_and_years_count_whole_periods_from_the_anchor() {
        let anchor = at("2026-01-01T06:00:00Z");

        assert_eq!(
            period_start(anchor, Period::Day, at("2026-01-03T05:59:59Z")).unwrap(),
            at("2026-01-02T06:00:00Z")
        );
        assert_eq!(
            period_start(anchor, Period::Week, at("2026-01-09T00:00:00Z")).unwrap(),
            at("2026-01-08T06:00:00Z")
        );
        assert_eq!(
            period_start(anchor, Period::Year, at("2027-01-01T05:00:00Z")).unwrap(),
            anchor
        );
    }
}
