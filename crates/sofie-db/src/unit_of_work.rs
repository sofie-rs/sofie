use std::any::{Any, TypeId};
use std::sync::Arc;

use sofie_core::{Cast, ColumnReader, Error, Result, RowReader, SqlValue, Uuid};

use crate::aggregate::{Snapshot, Table, table};
use crate::sql::same_values;
use crate::type_cache::TypeCache;
use crate::{Aggregate, Dialect, Outcome, Plan, Reach, Reply, Stamp, Step, Write};

impl<A: Aggregate> Outcome for A {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.raise(self.meta().events());
        let Some(pending) = pending_write(self, plan.stamp()) else {
            return Ok(());
        };
        #[cfg_attr(not(feature = "children"), expect(unused_variables))]
        let owned_through_parent = refuse_beyond_reach(self, plan.reach())?;
        #[cfg(feature = "module-roles")]
        plan.act_as(A::MODULE);
        #[cfg(feature = "children")]
        crate::children::check_loaded_parent(self, plan, owned_through_parent);
        let meta = self.meta();
        if let (Some(loaded), false) = (meta.snapshot.as_ref(), pending.changed) {
            plan.push(update::<A>(meta.id(), loaded, pending.row, plan.dialect()));
            return Ok(());
        }
        let written = pending.row;
        let write = Write {
            table: table::<A>(),
            id: meta.id(),
            owner: self.owner(),
            stamp: plan.stamp(),
            loaded: meta.snapshot.as_ref(),
            fields: &written.fields,
            stored: &written.stored,
        };
        for field in A::UNIQUE {
            let index = format!("{}_{field}", write.table.name);
            plan.unique_column(A::MODULE, &index, field);
        }
        for (behavior, _) in &write.table.behaviors {
            behavior.planned(&write, plan)?;
        }
        #[cfg(any(feature = "contracts", feature = "publishing"))]
        crate::change_event::plan_change_event(self, &write, plan)?;
        let step = match meta.snapshot.as_ref() {
            None => insert::<A>(meta.id(), written, plan.dialect()),
            Some(loaded) => {
                self.refuse_unlisted_moves(&mut RowReader::new(&loaded.fields))?;
                #[cfg(feature = "children")]
                crate::children::refuse_unlisted_moves(self)?;
                update::<A>(meta.id(), loaded, written, plan.dialect())
            }
        };
        plan.push(step);
        #[cfg(feature = "cache")]
        plan.changed(crate::ChangedRecord {
            module: A::MODULE,
            aggregate: A::NAME,
            id: Some(meta.id()),
        });
        #[cfg(feature = "live")]
        plan_live_changes(self, plan);
        #[cfg(feature = "children")]
        crate::children::plan_children(self, plan, owned_through_parent);
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        let written = pending_write(self, stamp).map(|pending| pending.row);
        self.meta_mut().written(written);
        #[cfg(feature = "children")]
        crate::children::committed(self);
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(serde_json::json!({
            "id": self.meta().id().to_string(),
            "version": self.meta().version(),
        })))
    }
}

/// A record handed to another owner leaves the screens of the one it was loaded with too.
#[cfg(feature = "live")]
fn plan_live_changes<A: Aggregate>(aggregate: &A, plan: &mut Plan<'_>) {
    let owner = aggregate.owner();
    plan.live_change(crate::live::TableChange::of::<A>(owner, plan.reach()));
    let loaded = aggregate
        .meta()
        .snapshot
        .as_ref()
        .and_then(loaded_owner::<A>);
    if loaded.is_some() && loaded != owner {
        plan.live_change(crate::live::TableChange::of::<A>(loaded, plan.reach()));
    }
}

/// The row a save writes. Unchanged but raising events, a loaded record still takes the next
/// version, so its events commit only from the state they were raised in.
struct Pending {
    row: Snapshot,
    changed: bool,
}

/// The same stamp gives the same answer, so planning and recording agree.
fn pending_write<A: Aggregate>(aggregate: &A, stamp: Stamp) -> Option<Pending> {
    let table = table::<A>();
    let meta = aggregate.meta();
    let mut fields = Vec::with_capacity(table.columns.len());
    aggregate.encode(&mut fields);
    let mut stored = meta.stored.clone();
    stored.resize(table.stored.len(), SqlValue::Null);
    let loaded = meta.snapshot.as_ref();
    let unchanged = loaded.filter(|loaded| {
        same_values(&loaded.fields, &fields) && same_values(&loaded.stored, &stored)
    });
    #[cfg(feature = "children")]
    let unchanged = unchanged.filter(|_| !crate::children::children_changed(aggregate));
    if let Some(loaded) = unchanged {
        return (!meta.events().is_empty()).then(|| Pending {
            row: next_version(loaded),
            changed: false,
        });
    }
    let write = Write {
        table,
        id: meta.id(),
        owner: aggregate.owner(),
        stamp,
        loaded,
        fields: &fields,
        stored: &[],
    };
    for (behavior, range) in &table.behaviors {
        behavior.written(&write, &mut stored[range.clone()]);
    }
    Some(Pending {
        row: Snapshot { stored, fields },
        changed: true,
    })
}

/// `version` is the first stored column.
fn next_version(loaded: &Snapshot) -> Snapshot {
    let mut next = loaded.clone();
    next.stored[0] = SqlValue::Int(crate::behavior::version(loaded.stored.first()) + 1);
    next
}

/// A record owned through its parent is checked in the database, as loaded and as written.
fn refuse_beyond_reach<A: Aggregate>(aggregate: &A, reach: Reach) -> Result<Option<Uuid>> {
    let Reach::OwnedBy(owner) = reach else {
        return Ok(None);
    };
    let taken_over = aggregate
        .meta()
        .snapshot
        .as_ref()
        .and_then(loaded_owner::<A>)
        .is_some_and(|loaded| loaded != owner);
    match aggregate.owner() {
        Some(own) if own == owner && !taken_over => Ok(None),
        #[cfg(feature = "children")]
        None if A::OWNER.is_some() => Ok(Some(owner)),
        _ => Err(Error::internal_message(format!(
            "an owned area wrote a `{}` its principal does not own; it may only read \
             records of others and records without an owner",
            A::NAME
        ))),
    }
}

fn loaded_owner<A: Aggregate>(loaded: &Snapshot) -> Option<Uuid> {
    let index = table::<A>()
        .columns
        .iter()
        .position(|column| Some(column.as_str()) == A::OWNER)?;
    match loaded.fields.get(index) {
        Some(SqlValue::Uuid(owner)) => Some(*owner),
        _ => None,
    }
}

static INSERTS: TypeCache<(TypeId, TypeId), Arc<str>> = TypeCache::new();

/// Its text depends only on the type and the dialect, so it is built once for both.
fn insert<A: Aggregate>(id: Uuid, written: Snapshot, dialect: &dyn Dialect) -> Step {
    let key = (TypeId::of::<A>(), (dialect as &dyn Any).type_id());
    let sql = INSERTS.get_or_insert_with(key, || {
        let table = table::<A>();
        let columns: Vec<&str> = table.row_columns().collect();
        crate::sql::insert(dialect, &table.qualified(dialect), &columns).into()
    });
    let mut params = vec![SqlValue::Uuid(id)];
    params.extend(written.stored);
    params.extend(written.fields);
    Step::statement(&*sql, params, None)
}

static UPDATES: TypeCache<(TypeId, TypeId), Arc<str>> = TypeCache::new();

/// Every column, only where the row still has the guarded columns it was loaded with; one text
/// per type and dialect, as for inserts.
fn update<A: Aggregate>(
    id: Uuid,
    loaded: &Snapshot,
    written: Snapshot,
    dialect: &dyn Dialect,
) -> Step {
    let table = table::<A>();
    let key = (TypeId::of::<A>(), (dialect as &dyn Any).type_id());
    let sql = UPDATES.get_or_insert_with(key, || update_sql(table, loaded, dialect).into());
    let mut params = written.stored;
    params.extend(written.fields);
    params.push(SqlValue::Uuid(id));
    params.extend(guarded(table, loaded).map(|(_, value)| value.clone()));
    Step::statement(&*sql, params, Some(1))
}

fn update_sql(table: &Table, loaded: &Snapshot, dialect: &dyn Dialect) -> String {
    let mut index = 0;
    let mut next = |column: &str| {
        index += 1;
        format!("{} = {}", dialect.quote(column), dialect.placeholder(index))
    };
    let assignments: Vec<String> = table
        .stored
        .iter()
        .chain(&table.columns)
        .map(String::as_str)
        .map(&mut next)
        .collect();
    let conditions: Vec<String> = std::iter::once("id")
        .chain(guarded(table, loaded).map(|(column, _)| column))
        .map(next)
        .collect();
    format!(
        "UPDATE {} SET {} WHERE {}",
        table.qualified(dialect),
        assignments.join(", "),
        conditions.join(" AND "),
    )
}

fn guarded<'a>(
    table: &'a Table,
    loaded: &'a Snapshot,
) -> impl Iterator<Item = (&'a str, &'a SqlValue)> {
    table
        .behaviors
        .iter()
        .filter(|(behavior, _)| behavior.guards())
        .flat_map(move |(_, range)| {
            table.stored[range.clone()]
                .iter()
                .map(String::as_str)
                .zip(&loaded.stored[range.clone()])
        })
}

pub fn refuse_unlisted_move<T: Cast>(
    field: &str,
    current: &T,
    loaded: &mut dyn ColumnReader,
) -> Result<()> {
    if !T::HOLDS_STATES {
        return skip(loaded, T::COLUMNS.len());
    }
    let stored = T::decode(loaded)?;
    T::refuse_unlisted_move(field, &stored, current)
}

fn skip(loaded: &mut dyn ColumnReader, columns: usize) -> Result<()> {
    for _ in 0..columns {
        loaded.skip()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sofie_core::moves::{Moves, Targets};
    use sofie_core::{ColumnSpec, SqlType, Variants, variants};

    use sofie_core::Instant;

    use super::*;
    use crate::aggregate::tests::{NUMBERED, Note};
    use crate::{Field, Meta, SoftDeletes, Source};

    #[derive(Clone, Copy, PartialEq)]
    enum Light {
        Green,
        Red,
    }

    impl Cast for Light {
        const COLUMNS: &'static [ColumnSpec] = &[ColumnSpec::new("", SqlType::Text)];
        const MOVES: Option<Moves<Self>> = Some(|light| {
            Some(match light {
                Self::Green => &Targets {
                    of: "Light",
                    states: &["RED"],
                },
                Self::Red => &Targets {
                    of: "Light",
                    states: &[],
                },
            })
        });

        fn encode(&self, out: &mut Vec<SqlValue>) {
            out.push(SqlValue::Text(self.name().to_owned()));
        }

        fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
            match row.text()? {
                "GREEN" => Ok(Self::Green),
                _ => Ok(Self::Red),
            }
        }
    }

    impl Light {
        fn name(self) -> &'static str {
            match self {
                Self::Green => "GREEN",
                Self::Red => "RED",
            }
        }
    }

    fn refused_move<T: Cast + Copy>(field: &str, from: T, to: T) -> Result<()> {
        let mut values = Vec::new();
        from.encode(&mut values);
        refuse_unlisted_move(field, &to, &mut RowReader::new(&values))
    }

    fn refused_stage_move(to: Stage) -> Result<()> {
        refused_move("stage", Stage::Open, to)
    }

    #[test]
    fn a_listed_move_and_no_move_pass() {
        assert!(refused_move("light", Light::Green, Light::Red).is_ok());
        assert!(refused_move("light", Light::Red, Light::Red).is_ok());
    }

    #[test]
    fn an_unlisted_move_is_refused_by_the_rule() {
        let refused = refused_move("light", Light::Red, Light::Green);

        let Err(Error::Invalid(fields)) = &refused else {
            panic!("expected a field error, got {refused:?}");
        };
        assert_eq!(
            (
                fields[0].field.as_str(),
                fields[0].key.as_ref(),
                fields[0].message.as_str()
            ),
            ("light", "validation.move", "cannot move from RED to GREEN")
        );
        assert_eq!(
            fields[0].arguments,
            [
                ("from".into(), "RED".to_owned()),
                ("to".into(), "GREEN".to_owned())
            ]
        );
    }

    #[derive(Clone, Copy)]
    struct Signal {
        light: Light,
        count: i64,
    }

    /// As `#[derive(Value)]` writes it for several fields.
    impl Cast for Signal {
        const COLUMNS: &'static [ColumnSpec] = &[
            ColumnSpec::new("_light", SqlType::Text),
            ColumnSpec::new("_count", SqlType::Int),
        ];
        const HOLDS_STATES: bool = Light::HOLDS_STATES || i64::HOLDS_STATES;

        fn encode(&self, out: &mut Vec<SqlValue>) {
            self.light.encode(out);
            self.count.encode(out);
        }

        fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                light: Cast::decode(row)?,
                count: Cast::decode(row)?,
            })
        }

        fn refuse_unlisted_move(field: &str, stored: &Self, current: &Self) -> Result<()> {
            Light::refuse_unlisted_move(field, &stored.light, &current.light)?;
            i64::refuse_unlisted_move(field, &stored.count, &current.count)
        }
    }

    #[test]
    fn a_state_inside_a_value_of_several_fields_moves_only_as_listed() {
        let signal = |light, count| Signal { light, count };

        assert!(refused_move("signal", signal(Light::Green, 1), signal(Light::Red, 2)).is_ok());
        assert!(refused_move("signal", signal(Light::Red, 1), signal(Light::Green, 1)).is_err());
        assert!(
            refused_move(
                "signal",
                Some(signal(Light::Red, 1)),
                Some(signal(Light::Green, 1))
            )
            .is_err()
        );
    }

    #[cfg(feature = "children")]
    struct StreetLamp {
        id: Uuid,
        light: Light,
    }

    #[cfg(feature = "children")]
    impl crate::Child for StreetLamp {
        const TABLE: crate::ChildTable = crate::ChildTable {
            module: "street",
            name: "lamp",
            fields: &[Field::new("light", Light::COLUMNS, false)],
        };

        fn id(&self) -> Uuid {
            self.id
        }

        fn encode(&self, out: &mut Vec<SqlValue>) {
            self.light.encode(out);
        }

        fn decode(id: Uuid, row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                id,
                light: Cast::decode(row)?,
            })
        }

        fn refuse_unlisted_moves(&self, loaded: &mut dyn ColumnReader) -> Result<()> {
            refuse_unlisted_move("light", &self.light, loaded)
        }
    }

    #[cfg(feature = "children")]
    #[test]
    fn a_changed_child_row_moves_its_state_only_as_listed() {
        use crate::ChildRows as _;

        let mut lamps = crate::Children::default();
        lamps.push(StreetLamp {
            id: Uuid::now_v7().unwrap(),
            light: Light::Red,
        });
        lamps.committed();
        lamps[0].light = Light::Green;

        assert!(lamps.refuse_unlisted_moves().is_err());
        lamps.push(StreetLamp {
            id: Uuid::now_v7().unwrap(),
            light: Light::Green,
        });
        lamps[0].light = Light::Red;
        assert!(lamps.refuse_unlisted_moves().is_ok());
    }

    #[test]
    fn a_value_without_transitions_may_change_freely() {
        let values = [SqlValue::Int(1)];

        assert!(refuse_unlisted_move("count", &2_i64, &mut RowReader::new(&values)).is_ok());
    }

    #[derive(Clone, Copy)]
    enum Stage {
        Open,
        Paid,
        Shipped,
    }

    const STAGE: &str = "unit_of_work::Stage";

    impl Variants for Stage {
        const TYPE: &'static str = "Stage";
        const PATH: &'static str = STAGE;
        const VARIANTS: &'static [(&'static str, Self)] = &[
            ("OPEN", Self::Open),
            ("PAID", Self::Paid),
            ("SHIPPED", Self::Shipped),
        ];
        const TRANSITIONS: &'static [(Self, Self)] =
            &[(Self::Open, Self::Paid), (Self::Paid, Self::Shipped)];
    }

    impl Cast for Stage {
        const COLUMNS: &'static [ColumnSpec] = variants::COLUMNS;
        const MOVES: Option<Moves<Self>> = Some(|stage| {
            Some(match stage {
                Self::Open => &Targets {
                    of: STAGE,
                    states: &["PAID"],
                },
                Self::Paid => &Targets {
                    of: STAGE,
                    states: &["SHIPPED"],
                },
                Self::Shipped => &Targets {
                    of: STAGE,
                    states: &[],
                },
            })
        });

        fn encode(&self, out: &mut Vec<SqlValue>) {
            variants::encode(*self, out);
        }

        fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
            variants::decode(row)
        }
    }

    #[test]
    fn moves_made_on_two_records_do_not_explain_a_third_s_jump() {
        let paid = Stage::Open.to(Stage::Paid).unwrap();
        let shipped = Stage::Paid.to(Stage::Shipped).unwrap();

        assert!(refused_move("stage", Stage::Open, paid).is_ok());
        assert!(refused_move("stage", Stage::Paid, shipped).is_ok());
        assert!(matches!(
            refused_stage_move(Stage::Shipped),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn two_moves_made_through_to_pass_where_the_same_jump_by_assignment_is_refused() {
        assert!(matches!(
            refused_stage_move(Stage::Shipped),
            Err(Error::Invalid(_))
        ));

        let shipped = Stage::Open
            .to(Stage::Paid)
            .and_then(|paid| paid.to(Stage::Shipped));

        assert!(refused_stage_move(shipped.unwrap()).is_ok());
    }

    fn now() -> Instant {
        Instant::from(sofie_core::Timestamp::from_second(1_000).unwrap())
    }

    fn planned(outcome: &impl Outcome, now: Instant) -> Result<Vec<(String, Vec<SqlValue>)>> {
        let mut plan = Plan::new(&NUMBERED, Reach::All, now, Vec::new(), None);
        outcome.plan(&mut plan)?;
        Ok(plan
            .into_parts()?
            .steps
            .into_iter()
            .map(|step| match step {
                Step::Statement { sql, params, .. } => (sql, params),
                Step::Script(sql) => (sql, Vec::new()),
            })
            .collect())
    }

    fn loaded_note(title: &str) -> Note {
        let created = SqlValue::Timestamp(sofie_core::Timestamp::from_second(10).unwrap());
        let values = [
            SqlValue::Uuid(Uuid::from_u128(1)),
            SqlValue::Int(4),
            created.clone(),
            created,
            SqlValue::Null,
            SqlValue::Text(title.to_owned()),
        ];
        <Note as Source>::decode(&mut RowReader::new(&values)).unwrap()
    }

    #[test]
    fn an_insert_writes_the_behaviors_columns_at_the_commits_time() {
        let note = Note {
            meta: Meta::new().unwrap(),
            title: "draft".to_owned(),
        };

        let steps = planned(&note, now()).unwrap();

        assert_eq!(
            steps[0].0,
            "INSERT INTO \"notes\".\"note\" (\"id\", \"version\", \"created_at\", \"updated_at\", \
             \"deleted_at\", \"title\") VALUES ($1, $2, $3, $4, $5, $6)"
        );
        assert_eq!(
            steps[0].1[1..5],
            [
                SqlValue::Int(1),
                SqlValue::Timestamp(now().timestamp()),
                SqlValue::Timestamp(now().timestamp()),
                SqlValue::Null
            ]
        );
    }

    #[test]
    fn an_update_writes_every_column_and_counts_the_version_up_where_it_is_unchanged() {
        let mut note = loaded_note("draft");
        note.title = "final".to_owned();

        let steps = planned(&note, now()).unwrap();

        assert_eq!(
            steps[0].0,
            "UPDATE \"notes\".\"note\" SET \"version\" = $1, \"created_at\" = $2, \
             \"updated_at\" = $3, \"deleted_at\" = $4, \"title\" = $5 \
             WHERE \"id\" = $6 AND \"version\" = $7"
        );
        assert_eq!(steps[0].1[0], SqlValue::Int(5));
        assert_eq!(steps[0].1[2], SqlValue::Timestamp(now().timestamp()));
        assert_eq!(steps[0].1[6], SqlValue::Int(4));
    }

    struct Noted;

    impl crate::Input for Noted {
        fn from_json(_value: serde_json::Value) -> Result<Self> {
            Ok(Self)
        }
    }

    impl serde::Serialize for Noted {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_unit_struct("Noted")
        }
    }

    impl crate::Event for Noted {
        const NAME: &'static str = "notes.Noted";
    }

    #[test]
    fn an_event_raised_without_a_change_commits_only_from_the_loaded_version() {
        let mut note = loaded_note("draft");
        note.meta.raise(Noted).unwrap();

        let mut plan = Plan::new(&NUMBERED, Reach::All, now(), Vec::new(), None);
        note.plan(&mut plan).unwrap();
        let steps = plan.into_parts().unwrap().steps;
        note.committed(Stamp {
            now: now(),
            actor: None,
            action: None,
        });

        let Step::Statement {
            sql,
            params,
            expect_rows,
        } = &steps[0]
        else {
            panic!("expected the version-checked update first, got {steps:?}");
        };
        assert!(sql.starts_with("UPDATE \"notes\".\"note\""), "{sql}");
        assert_eq!(params[0], SqlValue::Int(5));
        assert_eq!(params[2], loaded_note("draft").meta.stored[2]);
        assert_eq!(params[6], SqlValue::Int(4));
        assert_eq!(*expect_rows, Some(crate::ExpectedRows::or_conflict(1)));
        assert_eq!(steps.len(), 2, "the update, then the event");
        assert_eq!(note.meta().version(), 5);
        assert_eq!(planned(&note, now()).unwrap().len(), 0);
    }

    #[test]
    fn an_event_on_a_record_beyond_reach_is_refused_without_a_change() {
        let mut note = loaded_note("draft");
        note.meta.raise(Noted).unwrap();

        let mut plan = Plan::new(
            &NUMBERED,
            Reach::OwnedBy(Uuid::from_u128(9)),
            now(),
            Vec::new(),
            None,
        );

        assert!(matches!(note.plan(&mut plan), Err(Error::Internal { .. })));
    }

    #[test]
    fn a_float_is_written_when_its_bits_change_and_not_when_they_stay() {
        assert!(same_values(
            &[SqlValue::Float(f64::NAN)],
            &[SqlValue::Float(f64::NAN)]
        ));
        assert!(!same_values(
            &[SqlValue::Float(0.0)],
            &[SqlValue::Float(-0.0)]
        ));
    }

    #[test]
    fn a_deletion_is_stamped_and_recorded_once_committed() {
        let mut note = loaded_note("draft");
        note.delete();

        let steps = planned(&note, now()).unwrap();
        note.committed(Stamp {
            now: now(),
            actor: None,
            action: None,
        });

        assert!(steps[0].0.contains("\"deleted_at\" = $4"), "{}", steps[0].0);
        assert_eq!(steps[0].1[3], SqlValue::Timestamp(now().timestamp()));
        assert_eq!(note.deleted_at(), Some(now()));
        assert_eq!(note.meta().version(), 5);
        assert_eq!(planned(&note, now()).unwrap().len(), 0);
        note.restore();
        assert!(!note.is_deleted());
        assert_eq!(planned(&note, now()).unwrap().len(), 1);
    }

    struct Lamp {
        meta: Meta,
        light: Light,
    }

    impl Aggregate for Lamp {
        const NAME: &'static str = "lamp";
        const MODULE: &'static str = "lamps";
        const FIELDS: &'static [Field] = &[Field::new("light", Light::COLUMNS, false)];

        fn meta(&self) -> &Meta {
            &self.meta
        }

        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }

        fn encode(&self, out: &mut Vec<SqlValue>) {
            self.light.encode(out);
        }

        fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                meta,
                light: Light::decode(row)?,
            })
        }

        fn refuse_unlisted_moves(&self, loaded: &mut dyn ColumnReader) -> Result<()> {
            refuse_unlisted_move("light", &self.light, loaded)
        }
    }

    #[test]
    fn a_save_along_an_unlisted_edge_is_refused_whatever_changed_the_field() {
        let values = [
            SqlValue::Uuid(Uuid::from_u128(1)),
            SqlValue::Int(1),
            SqlValue::Text("RED".to_owned()),
        ];
        let mut lamp = <Lamp as Source>::decode(&mut RowReader::new(&values)).unwrap();
        lamp.light = Light::Green;

        assert!(matches!(planned(&lamp, now()), Err(Error::Invalid(_))));
        assert_eq!(lamp.meta.version(), 1);
    }

    #[cfg(feature = "live")]
    struct Card {
        meta: Meta,
        holder: Uuid,
    }

    #[cfg(feature = "live")]
    impl Aggregate for Card {
        const NAME: &'static str = "card";
        const MODULE: &'static str = "cards";
        const FIELDS: &'static [Field] = &[Field::new("holder", Uuid::COLUMNS, false)];
        const OWNER: Option<&'static str> = Some("holder");

        fn meta(&self) -> &Meta {
            &self.meta
        }

        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }

        fn owner(&self) -> Option<Uuid> {
            Some(self.holder)
        }

        fn encode(&self, out: &mut Vec<SqlValue>) {
            self.holder.encode(out);
        }

        fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                meta,
                holder: Uuid::decode(row)?,
            })
        }
    }

    #[cfg(feature = "live")]
    #[test]
    fn a_record_handed_to_another_owner_is_announced_to_both() {
        let (before, after) = (Uuid::from_u128(7), Uuid::from_u128(8));
        let values = [
            SqlValue::Uuid(Uuid::from_u128(1)),
            SqlValue::Int(1),
            SqlValue::Uuid(before),
        ];
        let mut card = <Card as Source>::decode(&mut RowReader::new(&values)).unwrap();
        card.holder = after;

        let mut plan = Plan::new(&NUMBERED, Reach::All, now(), Vec::new(), None);
        card.plan(&mut plan).unwrap();
        let readers: Vec<crate::Readers> = plan
            .into_parts()
            .unwrap()
            .live
            .iter()
            .map(|change| change.readers)
            .collect();

        assert_eq!(
            readers,
            [crate::Readers::Owner(after), crate::Readers::Owner(before)]
        );
    }

    #[cfg(feature = "live")]
    #[test]
    fn a_raw_statement_touching_a_table_is_announced_to_its_owner() {
        let holder = Uuid::from_u128(7);
        let mut plan = Plan::new(&NUMBERED, Reach::All, now(), Vec::new(), None);
        plan.push(Step::statement(
            "UPDATE cards.card SET holder = holder",
            Vec::new(),
            None,
        ));
        plan.touched::<Card>(Some(holder));
        let parts = plan.into_parts().unwrap();

        assert_eq!(
            parts.live,
            [crate::TableChange {
                topic: crate::live::Topic::of::<Card>(),
                readers: crate::Readers::Owner(holder),
            }]
        );
        #[cfg(feature = "cache")]
        assert!(matches!(
            parts.changed[..],
            [crate::ChangedRecord {
                module: "cards",
                aggregate: "card",
                id: None
            }]
        ));
    }
}
