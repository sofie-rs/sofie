use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{ParseStream, Parser};
use syn::{Ident, LitStr, Path, Token};

/// `sofie gen` reads the schedule; the expansion only names what rustc can check: the `each`
/// query, so a typo fails to compile, and a wall-clock time and weekday, evaluated in a constant
/// by sofie-values' own parser, so a wrong one fails at the attribute.
pub(crate) fn attribute(arguments: TokenStream, item: TokenStream) -> TokenStream {
    match read.parse2(arguments).and_then(Arguments::checked) {
        Ok(arguments) => {
            let checks = arguments.checks();
            if checks.is_empty() {
                return item;
            }
            quote! {
                #item
                const _: () = {
                    #checks
                };
            }
        }
        Err(error) => {
            let error = error.into_compile_error();
            quote!(#error #item)
        }
    }
}

#[derive(Default)]
struct Arguments {
    interval: Option<LitStr>,
    each: Option<Path>,
    daily_at: Option<LitStr>,
    weekly_on: Option<LitStr>,
    zone: Option<LitStr>,
}

fn read(input: ParseStream) -> syn::Result<Arguments> {
    let mut arguments = Arguments::default();
    if input.peek(LitStr) {
        arguments.interval = Some(input.parse()?);
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
        }
    }
    while !input.is_empty() {
        let key: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let slot = match key.to_string().as_str() {
            "each" => {
                set(&mut arguments.each, &key, input.parse()?)?;
                None
            }
            "daily_at" => Some(&mut arguments.daily_at),
            "weekly_on" => Some(&mut arguments.weekly_on),
            "zone" => Some(&mut arguments.zone),
            _ => {
                return Err(syn::Error::new(
                    key.span(),
                    "expected `each`, `daily_at`, `weekly_on` or `zone`",
                ));
            }
        };
        if let Some(slot) = slot {
            set(slot, &key, input.parse()?)?;
        }
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
        }
    }
    Ok(arguments)
}

fn set<T>(slot: &mut Option<T>, key: &Ident, value: T) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new(
            key.span(),
            format!("`{key}` is given twice"),
        ));
    }
    Ok(())
}

impl Arguments {
    fn checked(self) -> syn::Result<Self> {
        let refuse = |message: &str| Err(syn::Error::new(proc_macro2::Span::call_site(), message));
        match (&self.interval, &self.daily_at) {
            (None, None) => refuse(
                "give an interval such as \"1h\", or `daily_at = \"03:00\", zone = \"Europe/Istanbul\"`",
            ),
            (Some(_), Some(_)) => refuse("an interval and `daily_at` exclude each other"),
            (Some(_), None) if self.zone.is_some() || self.weekly_on.is_some() => {
                refuse("`zone` and `weekly_on` go with `daily_at`, not with an interval")
            }
            (None, Some(_)) if self.zone.is_none() => refuse(
                "`daily_at` needs `zone`, the IANA time zone whose clock it reads, such as \"Europe/Istanbul\"",
            ),
            (None, Some(_)) if self.each.is_some() => {
                refuse("`each` goes with an interval, not with `daily_at`")
            }
            _ => Ok(self),
        }
    }

    fn checks(&self) -> TokenStream {
        let each = self.each.as_ref().map(|path| quote!(let _ = #path;));
        let Some(time) = &self.daily_at else {
            return quote!(#each);
        };
        let time =
            quote_spanned!(time.span() => let _ = ::sofie::values::TimeOfDay::constant(#time););
        let weekday = self.weekly_on.as_ref().map(|weekday| {
            quote_spanned!(weekday.span() => let _ = ::sofie::values::Weekday::constant(#weekday);)
        });
        quote!(::sofie::__needs_local_time! { #time #weekday })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(arguments: TokenStream) -> String {
        attribute(
            arguments,
            quote!(
                pub async fn tidy() {}
            ),
        )
        .to_string()
    }

    fn refusal(arguments: TokenStream) -> String {
        let expanded = expanded(arguments);
        assert!(expanded.contains("compile_error"), "{expanded}");
        expanded
    }

    #[test]
    fn the_expansion_names_the_each_query_so_rustc_resolves_it() {
        let expanded = attribute(
            quote!("1h", each = queries::late),
            quote!(
                pub async fn remind() {}
            ),
        );

        assert_eq!(
            expanded.to_string(),
            quote!(
                pub async fn remind() {}
                const _: () = {
                    let _ = queries::late;
                };
            )
            .to_string()
        );
    }

    #[test]
    fn a_schedule_with_nothing_to_check_is_left_as_written() {
        let item = quote!(
            pub async fn expire() {}
        );

        assert_eq!(
            attribute(quote!("15m"), item.clone()).to_string(),
            item.to_string()
        );
    }

    #[test]
    fn a_wall_clock_time_and_weekday_are_evaluated_by_their_values_behind_the_local_time_feature() {
        let expanded = expanded(quote!(
            weekly_on = "monday",
            daily_at = "03:00",
            zone = "Europe/Istanbul"
        ));

        assert!(
            expanded.contains(
                ":: sofie :: __needs_local_time ! { let _ = :: sofie :: values :: TimeOfDay :: constant (\"03:00\") ;"
            ),
            "{expanded}"
        );
        assert!(
            expanded.contains("let _ = :: sofie :: values :: Weekday :: constant (\"monday\") ;"),
            "{expanded}"
        );
    }

    #[test]
    fn a_schedule_takes_an_interval_or_a_daily_time_with_its_zone() {
        assert!(refusal(quote!()).contains("give an interval"));
        assert!(refusal(quote!("1h", daily_at = "03:00")).contains("exclude each other"));
        assert!(refusal(quote!(daily_at = "03:00")).contains("`daily_at` needs `zone`"));
        assert!(refusal(quote!("1h", zone = "UTC")).contains("go with `daily_at`"));
        assert!(refusal(quote!(weekly_on = "monday")).contains("give an interval"));
        let each = quote!(daily_at = "03:00", zone = "UTC", each = late);
        assert!(refusal(each).contains("`each` goes with an interval"));
    }

    #[test]
    fn an_unknown_or_repeated_argument_is_refused() {
        assert!(refusal(quote!(hourly_at = "03")).contains("expected `each`"));
        let twice = quote!(daily_at = "03:00", daily_at = "04:00", zone = "UTC");
        assert!(refusal(twice).contains("`daily_at` is given twice"));
        assert!(refusal(quote!(15)).contains("expected identifier"));
    }

    #[test]
    fn the_function_is_kept_beside_a_refusal() {
        assert!(refusal(quote!()).contains("pub async fn tidy"));
    }
}
