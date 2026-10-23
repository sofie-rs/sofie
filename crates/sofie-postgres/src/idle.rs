use std::time::Duration;

use deadpool_postgres::Pool;

const IDLE_LIMIT: Duration = Duration::from_mins(5);
const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// Kept warm so the first request after a quiet spell does not pay for a new
/// connection and its prepared statements.
const KEEP_OPEN: usize = 2;
/// Clouds' NAT gateways and load balancers drop idle TCP after a few minutes
/// (AWS NAT: 350 s); the 2 hour default would leave the kept connections dead.
pub(crate) const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);

/// A pool built outside a runtime cannot open connections, so it has none to
/// close; the sweep ends with the pool.
pub(crate) fn close_idle_in_background(pool: &Pool) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let pool = pool.weak();
    runtime.spawn(async move {
        let mut sweeps = tokio::time::interval(SWEEP_EVERY);
        loop {
            sweeps.tick().await;
            let Some(pool) = pool.upgrade() else {
                return;
            };
            close_idle(&pool, IDLE_LIMIT);
        }
    });
}

fn close_idle(pool: &Pool, idle_limit: Duration) -> usize {
    let mut open = pool.status().size;
    let swept = pool.retain(|_, metrics| {
        if open <= KEEP_OPEN || metrics.last_used() < idle_limit {
            return true;
        }
        open -= 1;
        false
    });
    swept.removed.len()
}

#[cfg(test)]
mod tests {
    use sofie_testing::TestDatabase;

    use super::*;
    use crate::Postgres;

    async fn open_four(pool: &Pool) {
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(pool.get().await.unwrap());
        }
        drop(held);
        assert_eq!(pool.status().size, 4);
    }

    #[tokio::test]
    async fn idle_connections_close_down_to_the_two_kept_open() {
        let database = TestDatabase::create().await;
        let postgres = Postgres::connect(database.url()).unwrap();
        open_four(&postgres.pool).await;

        assert_eq!(close_idle(&postgres.pool, Duration::ZERO), 2);
        assert_eq!(postgres.pool.status().size, 2);
        assert_eq!(close_idle(&postgres.pool, Duration::ZERO), 0);
        let kept = postgres.pool.get().await.unwrap();
        assert!(kept.simple_query("SELECT 1").await.is_ok());
    }

    #[tokio::test]
    async fn recently_used_connections_stay_open() {
        let database = TestDatabase::create().await;
        let postgres = Postgres::connect(database.url()).unwrap();
        open_four(&postgres.pool).await;

        assert_eq!(close_idle(&postgres.pool, IDLE_LIMIT), 0);
        assert_eq!(postgres.pool.status().size, 4);
    }
}
