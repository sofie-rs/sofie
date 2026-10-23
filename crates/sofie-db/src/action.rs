//! The route or job a commit is made for, which history records beside the actor.

tokio::task_local! {
    static ACTION: &'static str;
}

pub async fn acting_for<F: Future>(action: &'static str, work: F) -> F::Output {
    ACTION.scope(action, work).await
}

#[must_use]
pub fn current() -> Option<&'static str> {
    ACTION.try_with(|action| *action).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_action_is_known_only_inside_its_work() {
        let inside = acting_for("shop.web.place_order", async { current() }).await;

        assert_eq!(inside, Some("shop.web.place_order"));
        assert_eq!(current(), None);
    }
}
