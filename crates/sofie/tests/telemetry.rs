#![cfg(all(feature = "spans", feature = "capture"))]

use sofie::core::Result;

#[sofie::span]
async fn reserve_stock(count: u32) -> Result<u32> {
    if count == 0 {
        return Err(sofie::core::Error::NotFound);
    }
    Ok(count * 2)
}

#[sofie::span]
fn price(lines: i64) -> i64 {
    lines * 3
}

#[tokio::test]
async fn a_marked_step_is_a_span_named_after_its_path_without_its_arguments() {
    let telemetry = sofie::telemetry::capture();

    let reserved = reserve_stock(2).await.unwrap();
    let priced = price(1);

    assert_eq!((reserved, priced), (4, 3));
    let steps: Vec<_> = telemetry
        .spans()
        .into_iter()
        .filter(|span| span.name == "step")
        .map(|span| (span.fields["otel.name"].clone(), span.fields.len()))
        .collect();
    let named = |function: &str| (format!("telemetry::{function}"), 2);
    assert_eq!(steps, [named("reserve_stock"), named("price")]);
}

#[cfg(feature = "contracts")]
mod contract {
    use sofie::core::Result;

    #[sofie::contract]
    pub trait Prices: Send + Sync {
        async fn quote(&self, lines: i64) -> Result<i64>;
    }

    struct Fixed;

    #[sofie::contract]
    impl Prices for Fixed {
        async fn quote(&self, lines: i64) -> Result<i64> {
            Ok(lines * 5)
        }
    }

    #[tokio::test]
    async fn a_contract_call_is_a_span_named_after_the_contract_and_its_function() {
        let telemetry = sofie::telemetry::capture();
        let prices: Box<dyn Prices> = Box::new(Fixed);

        assert_eq!(prices.quote(2).await.unwrap(), 10);

        let calls: Vec<_> = telemetry
            .spans()
            .into_iter()
            .filter(|span| span.name == "contract")
            .map(|span| {
                (
                    span.fields["otel.name"].clone(),
                    span.fields["module"].clone(),
                )
            })
            .collect();
        assert_eq!(
            calls,
            [("Prices::quote".to_owned(), "telemetry".to_owned())]
        );
    }
}
