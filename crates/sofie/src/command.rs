use std::ffi::OsString;

use sofie_core::{DisplayZone, Error, Result};

const USAGE: &str = "usage: <app> [serve [--role web|worker|all]] | \
                     <app> flags settled (an app with feature flags) | \
                     <app> schema check | \
                     <app> db fresh | <app> db status | <app> serve --at INSTANT | \
                     <app> schedule run NAME (feature `development`) | \
                     <app> screen source KEY (an app with resources) | \
                     <app> task MODULE.NAME [--name value]... (feature `tasks`) | \
                     <app> jobs dead | <app> jobs replay ID | \
                     <app> report retention --start EVENT --returning EVENT [--period day|week|month] \
                     [--since DATE] (feature `product-reports`)";

const WORKER_USAGE: &str =
    "usage: <app>-worker [serve [--role worker]] | <app>-worker schema check";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Serve(Role),
    SchemaCheck,
    #[cfg(feature = "development")]
    ServeAt(sofie_core::Instant),
    #[cfg(feature = "resources")]
    ScreenSource(String),
    #[cfg(feature = "development")]
    FreshDatabase,
    #[cfg(feature = "development")]
    DatabaseStatus,
    /// Queues a schedule's job now, for the worker `sofie dev` runs.
    #[cfg(feature = "development")]
    RunSchedule(String),
    #[cfg(feature = "feature-flags")]
    SettledFlags,
    #[cfg(feature = "tasks")]
    Task {
        name: String,
        arguments: serde_json::Map<String, serde_json::Value>,
    },
    #[cfg(feature = "product-reports")]
    Report(crate::reports::Asked),
    Jobs(crate::jobs::Jobs),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Web,
    Worker,
    All,
}

impl Command {
    /// `zone` reads `serve --at` without an offset, as the toolbar's clock reads it.
    pub(crate) fn from_arguments(
        arguments: impl Iterator<Item = OsString>,
        zone: &DisplayZone,
    ) -> Result<Self> {
        Self::parse(&unicode(arguments, USAGE)?, zone)
    }

    #[cfg_attr(
        not(feature = "development"),
        expect(unused_variables, reason = "only `serve --at` reads the zone")
    )]
    fn parse(arguments: &[String], zone: &DisplayZone) -> Result<Self> {
        let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match words.as_slice() {
            [] | ["serve"] => Ok(Self::Serve(Role::All)),
            ["serve", "--role", role] => Role::parse(role).map(Self::Serve),
            ["schema", "check"] => Ok(Self::SchemaCheck),
            #[cfg(feature = "development")]
            ["db", "fresh"] => Ok(Self::FreshDatabase),
            #[cfg(feature = "development")]
            ["db", "status"] => Ok(Self::DatabaseStatus),
            #[cfg(feature = "development")]
            ["schedule", "run", name] => Ok(Self::RunSchedule((*name).to_owned())),
            #[cfg(feature = "development")]
            ["serve", "--at", instant] => {
                sofie_core::Instant::parse_in(instant, zone).map(Self::ServeAt)
            }
            #[cfg(feature = "resources")]
            ["screen", "source", key] => Ok(Self::ScreenSource((*key).to_owned())),
            #[cfg(feature = "feature-flags")]
            ["flags", "settled"] => Ok(Self::SettledFlags),
            #[cfg(feature = "product-reports")]
            ["report", "retention", options @ ..] => {
                crate::reports::Asked::parse(options).map(Self::Report)
            }
            #[cfg(feature = "tasks")]
            ["task", name, arguments @ ..] => Ok(Self::Task {
                name: (*name).to_owned(),
                arguments: crate::tasks::named_arguments(arguments)?,
            }),
            ["jobs", "dead"] => Ok(Self::Jobs(crate::jobs::Jobs::Dead)),
            ["jobs", "replay", id] => crate::jobs::Jobs::parse_replay(id).map(Self::Jobs),
            _ => Err(Error::internal_message(format!(
                "`{}` is not a command; {USAGE}",
                words.join(" ")
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerCommand {
    Serve,
    SchemaCheck,
}

impl WorkerCommand {
    pub(crate) fn from_arguments(arguments: impl Iterator<Item = OsString>) -> Result<Self> {
        Self::parse(&unicode(arguments, WORKER_USAGE)?)
    }

    fn parse(arguments: &[String]) -> Result<Self> {
        let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match words.as_slice() {
            [] | ["serve"] | ["serve", "--role", "worker"] => Ok(Self::Serve),
            ["schema", "check"] => Ok(Self::SchemaCheck),
            _ => Err(Error::internal_message(format!(
                "`{}` is not a command of a worker binary; {WORKER_USAGE}",
                words.join(" ")
            ))),
        }
    }
}

fn unicode(arguments: impl Iterator<Item = OsString>, usage: &str) -> Result<Vec<String>> {
    arguments
        .map(|argument| {
            argument.into_string().map_err(|_| {
                Error::internal_message(format!("an argument is not Unicode; {usage}"))
            })
        })
        .collect()
}

impl Role {
    fn parse(role: &str) -> Result<Self> {
        match role {
            "web" => Ok(Self::Web),
            "worker" => Ok(Self::Worker),
            "all" => Ok(Self::All),
            _ => Err(Error::internal_message(format!(
                "`{role}` is not a role; a process serves as `web`, `worker` or `all`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(arguments: &str) -> Vec<String> {
        arguments.split_whitespace().map(str::to_owned).collect()
    }

    fn parse(arguments: &str) -> Result<Command> {
        Command::parse(&words(arguments), &DisplayZone::named("UTC").unwrap())
    }

    #[test]
    fn a_binary_without_arguments_serves_both_roles() {
        assert_eq!(parse("").unwrap(), Command::Serve(Role::All));
        assert_eq!(parse("serve").unwrap(), Command::Serve(Role::All));
    }

    #[test]
    fn a_role_is_chosen_and_an_unknown_one_refused() {
        assert_eq!(
            parse("serve --role web").unwrap(),
            Command::Serve(Role::Web)
        );
        assert_eq!(
            parse("serve --role worker").unwrap(),
            Command::Serve(Role::Worker)
        );
        assert_eq!(
            parse("serve --role all").unwrap(),
            Command::Serve(Role::All)
        );

        let error = parse("serve --role cron").unwrap_err();
        assert!(
            error.to_string().starts_with("`cron` is not a role"),
            "{error}"
        );
    }

    #[test]
    fn the_schema_check_is_a_command_of_every_build() {
        assert_eq!(parse("schema check").unwrap(), Command::SchemaCheck);
    }

    #[cfg(feature = "development")]
    #[test]
    fn serving_at_an_instant_reads_it_in_the_display_zone_when_it_has_no_offset() {
        let istanbul = DisplayZone::named("Europe/Istanbul").unwrap();
        let instant = |text| sofie_core::Instant::parse(text).unwrap();

        let served = Command::parse(&words("serve --at 2027-01-01T09:00"), &istanbul).unwrap();

        assert_eq!(served, Command::ServeAt(instant("2027-01-01T06:00:00Z")));
        assert_eq!(
            parse("serve --at 2027-01-01T09:00+01:00").unwrap(),
            Command::ServeAt(instant("2027-01-01T08:00:00Z"))
        );
        assert!(parse("serve --at tomorrow").is_err());
    }

    #[cfg(feature = "resources")]
    #[test]
    fn a_screen_source_names_its_key() {
        assert_eq!(
            parse("screen source order/console/order_index").unwrap(),
            Command::ScreenSource("order/console/order_index".to_owned())
        );
    }

    #[cfg(feature = "development")]
    #[test]
    fn db_fresh_and_db_status_are_development_commands() {
        assert_eq!(parse("db fresh").unwrap(), Command::FreshDatabase);
        assert_eq!(parse("db status").unwrap(), Command::DatabaseStatus);
    }

    #[cfg(feature = "development")]
    #[test]
    fn a_schedule_is_run_by_its_name_in_development() {
        assert_eq!(
            parse("schedule run order.jobs.unpaid_orders").unwrap(),
            Command::RunSchedule("order.jobs.unpaid_orders".to_owned())
        );
        assert!(parse("schedule run").is_err());
    }

    #[cfg(feature = "tasks")]
    #[test]
    fn a_task_is_named_with_its_arguments() {
        let Command::Task { name, arguments } =
            parse("task order.backfill --since 2026-01-01").unwrap()
        else {
            panic!("not a task");
        };
        assert_eq!(name, "order.backfill");
        assert_eq!(arguments["since"], "2026-01-01");
        assert!(parse("task order.backfill --since").is_err());
    }

    #[test]
    fn a_dead_job_is_listed_and_replayed_by_its_id() {
        use crate::jobs::Jobs;
        let id = "0190a8f0-0000-7000-8000-000000000001";

        assert_eq!(parse("jobs dead").unwrap(), Command::Jobs(Jobs::Dead));
        assert_eq!(
            parse(&format!("jobs replay {id}")).unwrap(),
            Command::Jobs(Jobs::Replay(id.parse().unwrap()))
        );
        assert!(parse("jobs replay 42").is_err());
    }

    #[test]
    fn a_worker_binary_serves_only_jobs() {
        let worker = |arguments: &str| WorkerCommand::parse(&words(arguments));
        for serving in ["", "serve", "serve --role worker"] {
            assert_eq!(worker(serving).unwrap(), WorkerCommand::Serve);
        }
        assert_eq!(worker("schema check").unwrap(), WorkerCommand::SchemaCheck);

        let error = worker("serve --role web").unwrap_err();
        assert!(error.to_string().contains(WORKER_USAGE), "{error}");
    }

    #[test]
    fn anything_else_shows_the_usage() {
        let error = parse("db drop").unwrap_err();
        assert!(error.to_string().contains(USAGE), "{error}");
    }
}
