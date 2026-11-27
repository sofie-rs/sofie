use sofie::billing::{Limit, Plan, Plans, UNLIMITED};

pub const OPEN_TASKS: Limit = Limit::new("tracker.open_tasks");

pub const FREE: Plan = Plan::new("free").limits(&[(OPEN_TASKS, 3)]).seats(3);

pub const PRO: Plan = Plan::new("pro")
    .limits(&[(OPEN_TASKS, UNLIMITED)])
    .seats(25)
    .trial_days(14);

pub const PLANS: Plans = Plans::new(FREE, &[PRO]);
