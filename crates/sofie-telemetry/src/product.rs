//! Product events are never dropped by `SOFIE_LOG` or `SOFIE_LOG_SAMPLE`, so funnels count in full.

use std::ops::ControlFlow;
use std::sync::{Arc, OnceLock};

use sofie_core::{Timestamp, Uuid};
use tracing::field::{Field, Visit};
use tracing::span::Id;
use tracing::{Event, Metadata, Span};

use crate::Logged;
use crate::fields::{FieldValue, Fields};
use crate::subscriber::Telemetry;

pub const PRODUCT_TARGET: &str = "product";

pub(crate) type EvaluatedFlags = Arc<dyn Fn() -> String + Send + Sync>;

#[cfg(feature = "metrics")]
static PRODUCT_EVENTS: crate::metrics::Instrument = crate::metrics::Instrument {
    name: "product.events",
    description: "Product events recorded, by name.",
    kind: crate::metrics::Kind::Counter,
    labels: &["product.event"],
};

/// A product event as kept for reports: who, if a principal was signed in, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductRecord {
    pub name: String,
    pub principal: Option<Uuid>,
    pub at: Timestamp,
}

type Keep = Box<dyn Fn(ProductRecord) + Send + Sync>;

static KEEP: OnceLock<Keep> = OnceLock::new();

/// Hands every product event to `keep`, which must not block: it runs on the
/// thread that raised the event. Answers false when a keeper is set already.
pub fn keep_product_events(keep: impl Fn(ProductRecord) + Send + Sync + 'static) -> bool {
    KEEP.set(Box::new(keep)).is_ok()
}

/// Product events inside `span` belong to `principal`; it is kept, never logged.
pub fn attach_principal(span: &Span, principal: Uuid) {
    crate::with_telemetry(span, |telemetry, id| {
        telemetry
            .spans
            .with(id, |entry| entry.principal = Some(principal))
    });
}

pub(crate) fn is_product(metadata: &Metadata<'_>) -> bool {
    metadata.target() == PRODUCT_TARGET
}

#[derive(Default)]
struct Named(Option<String>);

impl Visit for Named {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "product.event" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl Telemetry {
    pub(crate) fn keep_product(&self, event: &Event<'_>, innermost: Option<&Id>) {
        let Some(keep) = KEEP.get() else {
            return;
        };
        let mut named = Named::default();
        event.record(&mut named);
        let Some(name) = named.0 else {
            return;
        };
        let mut principal = None;
        self.spans
            .walk_up(innermost, |entry| match entry.principal {
                Some(found) => {
                    principal = Some(found);
                    ControlFlow::Break(())
                }
                None => ControlFlow::Continue(()),
            });
        keep(ProductRecord {
            name,
            principal,
            at: Timestamp::now(),
        });
    }

    /// Rendered as the event is, so it lists the flags evaluated before it and no later ones.
    pub(crate) fn add_feature_flags(&self, innermost: Option<&Id>, fields: &mut Fields) {
        let mut nearest = None;
        self.spans
            .walk_up(innermost, |entry| match &entry.feature_flags {
                Some(evaluated) => {
                    nearest = Some(Arc::clone(evaluated));
                    ControlFlow::Break(())
                }
                None => ControlFlow::Continue(()),
            });
        let evaluated = nearest.map(|evaluated| evaluated()).unwrap_or_default();
        if !evaluated.is_empty() {
            fields.set("feature_flags", FieldValue::Text(evaluated));
        }
    }
}

impl Logged {
    #[doc(hidden)]
    #[cfg_attr(
        not(feature = "metrics"),
        expect(unused_variables, reason = "only metrics count events by name")
    )]
    pub fn emit_product(self, name: &'static str, event: impl FnOnce()) {
        #[cfg(feature = "metrics")]
        crate::metrics::record(&PRODUCT_EVENTS, &[name.into()], 1.0);
        self.emit(event);
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __product {
    ($name:expr ; [$($fields:tt)*]) => {
        $crate::__gather!([$($fields)*]).emit_product($name, || {
            $crate::__tracing::event!(
                target: $crate::PRODUCT_TARGET,
                $crate::__tracing::Level::INFO,
                product.event = $name,
            );
        })
    };
    ($name:expr ; [$($fields:tt)*] $head:ident $(. $rest:ident)* $(, $($tail:tt)*)?) => {
        $crate::__product!($name ; [$($fields)* ($head $($rest)*)] $($($tail)*)?)
    };
}

/// `product_event!(order.placed, order.total)`; logged under [`PRODUCT_TARGET`], never sampled.
#[macro_export]
macro_rules! product_event {
    ($first:ident $(. $rest:ident)* $(, $($fields:tt)+)?) => {
        $crate::__product!(
            concat!(stringify!($first) $(, ".", stringify!($rest))*) ; [] $($($fields)+)?
        )
    };
}

#[cfg(all(test, feature = "capture"))]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::Empty;

    use crate::context::Exporter;
    use crate::filter::LogFilter;
    use crate::line::LineFormat;
    use crate::sample::Sample;
    use crate::subscriber::{Output, Telemetry};
    use crate::writer::LogWriter;
    use crate::writer::tests::{Shared, written};

    #[test]
    fn a_product_event_is_written_whatever_the_filter_and_the_sample() {
        let out = Shared::default();
        let telemetry = Telemetry::new(
            Output::Lines {
                filter: LogFilter::parse("off").unwrap(),
                sample: Sample::parse("0").unwrap(),
                writer: LogWriter::start(LineFormat::Json, out.clone(), None).unwrap(),
            },
            Exporter::silent(),
        );

        tracing::subscriber::with_default(telemetry, || {
            let request = tracing::info_span!("request", trace_id = Empty);
            let _ = crate::continue_trace(&request, None);
            request.in_scope(|| {
                tracing::info!("dropped by the filter");
                crate::product_event!(order.placed);
            });
            tracing::dispatcher::get_default(|dispatch| {
                dispatch.downcast_ref::<Telemetry>().unwrap().flush();
            });
        });

        let text = written(&out);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains("\"product.event\":\"order.placed\""),
            "{text}"
        );
        assert!(text.contains("\"target\":\"product\""), "{text}");
    }

    /// The only test that sets the keeper, which is set once per process.
    #[test]
    fn a_kept_product_event_names_the_principal_its_flow_attached() {
        static KEPT: Mutex<Vec<super::ProductRecord>> = Mutex::new(Vec::new());
        let _capture = crate::capture();
        assert!(super::keep_product_events(|record| {
            KEPT.lock().unwrap().push(record);
        }));
        let principal = sofie_core::Uuid::from_u128(7);
        let request = tracing::info_span!("request");
        super::attach_principal(&request, principal);

        request.in_scope(|| {
            tracing::info_span!("handler").in_scope(|| crate::product_event!(kept.signed_in));
        });
        crate::product_event!(kept.anonymous);

        // Other tests raise product events at the same time.
        let kept: Vec<_> = KEPT
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.name.starts_with("kept."))
            .map(|record| (record.name.clone(), record.principal))
            .collect();
        assert_eq!(
            kept,
            [
                ("kept.signed_in".to_owned(), Some(principal)),
                ("kept.anonymous".to_owned(), None)
            ]
        );
        assert!(!super::keep_product_events(|_| {}));
    }

    #[test]
    fn a_product_event_carries_the_flags_its_flow_evaluated_before_it() {
        let capture = crate::capture();
        let evaluated = Arc::new(Mutex::new(String::new()));
        let request = tracing::info_span!("request");
        let answer = Arc::clone(&evaluated);
        crate::attach_feature_flags(&request, move || answer.lock().unwrap().clone());

        request.in_scope(|| {
            crate::product_event!(cart.opened);
            "order.layout=COMPACT".clone_into(&mut evaluated.lock().unwrap());
            tracing::info_span!("handler").in_scope(|| crate::product_event!(order.placed));
        });
        crate::product_event!(page.viewed);

        let flags: Vec<_> = capture
            .product_events()
            .into_iter()
            .map(|event| event.fields.get("feature_flags").cloned())
            .collect();
        assert_eq!(flags, [None, Some("order.layout=COMPACT".to_owned()), None]);
    }
}
