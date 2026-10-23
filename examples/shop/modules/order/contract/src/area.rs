use sofie::http::Area;
use sofie_identity::Account;

/// Shoppers with an account, each on their own orders.
pub struct Customer;

impl Area for Customer {
    const NAME: &'static str = "customer";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

impl sofie_identity::SignUp for Customer {
    const PATH: &'static str = "/sign-up";
}
