//! Statements for the table `migrations/0005_shield.sql` creates.

use sofie_db::ShieldSql;

pub(crate) const LISTEN: &str = "LISTEN sofie_shield";

// Expired rows are deleted by the next write rather than by a job, so the
// table holds at most the blocks of one period.
pub(crate) const SHIELD: ShieldSql = ShieldSql {
    upsert: "WITH expired AS (DELETE FROM sofie.shield WHERE until <= now())
INSERT INTO sofie.shield (key, rule, grade, until) VALUES ($1, $2, $3, $4)
ON CONFLICT (key) DO UPDATE SET rule = $2, grade = $3, until = $4",
    delete: "DELETE FROM sofie.shield WHERE key = $1",
    active: "SELECT key, rule, grade, until FROM sofie.shield WHERE until > $1",
    notify: "NOTIFY sofie_shield",
};
