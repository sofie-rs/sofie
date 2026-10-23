use sofie::http::ListedRoute;

/// `routes` paths spell out the member area's `{prefix}`, `{sign_in}` and `{sign_up}`.
const fn member(method: &'static str, path: &'static str, handler: &'static str) -> ListedRoute {
    ListedRoute::new("routes", method, path, handler)
}

#[cfg(any(
    feature = "permitted-screens",
    feature = "token-screens",
    feature = "impersonation"
))]
const fn permitted(
    mount: &'static str,
    method: &'static str,
    path: &'static str,
    handler: &'static str,
    permission: &'static str,
) -> ListedRoute {
    ListedRoute {
        permission: Some(permission),
        ..ListedRoute::new(mount, method, path, handler)
    }
}

#[rustfmt::skip]
pub const ROUTES: &[ListedRoute] = &[
    member("GET", "{sign_in}", "sign_in_page"),
    member("POST", "{sign_in}", "sign_in"),
    member("POST", "{prefix}/sign-out", "sign_out"),
    member("GET", "{sign_up}", "sign_up_page"),
    member("POST", "{sign_up}", "sign_up"),
    #[cfg(feature = "links")] member("GET", "{prefix}/link-sent", "link_sent_page"),
    #[cfg(feature = "links")] member("GET", "{prefix}/join/{account_link}", "join_page"),
    #[cfg(feature = "links")] member("POST", "{prefix}/join/{account_link}", "join"),
    #[cfg(feature = "password-reset")] member("GET", "{prefix}/forgot-password", "forgot_page"),
    #[cfg(feature = "password-reset")] member("POST", "{prefix}/forgot-password", "ask_reset"),
    #[cfg(feature = "password-reset")] member("GET", "{prefix}/reset-password/{account_link}", "reset_page"),
    #[cfg(feature = "password-reset")] member("POST", "{prefix}/reset-password/{account_link}", "reset_password"),
    #[cfg(feature = "email-verification")] member("GET", "{prefix}/verify-email", "ask_page"),
    #[cfg(feature = "email-verification")] member("POST", "{prefix}/verify-email", "ask_verification"),
    #[cfg(feature = "email-verification")] member("GET", "{prefix}/verify-email/{account_link}", "verify_page"),
    #[cfg(feature = "email-verification")] member("POST", "{prefix}/verify-email/{account_link}", "verify_email"),
    #[cfg(feature = "sign-in-links")] member("GET", "{prefix}/sign-in-link", "ask_sign_in_link_page"),
    #[cfg(feature = "sign-in-links")] member("POST", "{prefix}/sign-in-link", "ask_sign_in_link"),
    #[cfg(feature = "sign-in-links")] member("GET", "{prefix}/sign-in-link/{account_link}", "sign_in_link_page"),
    #[cfg(feature = "sign-in-links")] member("POST", "{prefix}/sign-in-link/{account_link}", "sign_in_by_link"),
    #[cfg(feature = "totp")] member("GET", "{prefix}/two-factor/{account_link}", "second_factor_page"),
    #[cfg(feature = "totp")] member("POST", "{prefix}/two-factor/{account_link}", "prove_second_factor"),
    #[cfg(feature = "passkeys")] member("POST", "{prefix}/passkey", "sign_in_with_passkey"),
    #[cfg(feature = "oauth")] member("POST", "{prefix}/oauth", "begin_oauth"),
    #[cfg(feature = "oauth")] member("GET", "{prefix}/oauth/callback", "oauth_return_page"),
    #[cfg(feature = "oauth")] member("POST", "{prefix}/oauth/callback", "finish_oauth"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "GET", "/roles", "list_roles", "sofie_identity.role.*.read"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "GET", "/roles/{role}", "show_role", "sofie_identity.role.*.read"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "GET", "/roles/new", "new_role", "sofie_identity.role.*.create"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "POST", "/roles/new", "create_role", "sofie_identity.role.*.create"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "GET", "/roles/{role}/edit", "edit_role", "sofie_identity.role.*.update"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "POST", "/roles/{role}/edit", "update_role", "sofie_identity.role.*.update"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "GET", "/roles/{role}/role-grants", "list_role_grants", "sofie_identity.role_grant.*.read"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "GET", "/roles/{role}/role-grants/new", "new_role_grant", "sofie_identity.role_grant.*.create"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "POST", "/roles/{role}/role-grants/new", "create_role_grant", "sofie_identity.role_grant.*.create"),
    #[cfg(feature = "permitted-screens")] permitted("permitted_routes", "POST", "/roles/{role}/role-grants/{role_grant}/delete", "delete_role_grant", "sofie_identity.role_grant.*.delete"),
    #[cfg(feature = "permitted-flags")] permitted("permitted_routes", "GET", "/flags", "list_flags", "sofie_identity.flag.*.read"),
    #[cfg(feature = "permitted-flags")] permitted("permitted_routes", "POST", "/flags", "target_flag", "sofie_identity.flag.*.update"),
    #[cfg(feature = "permitted-flags")] permitted("permitted_routes", "GET", "/settings", "list_settings", "sofie_identity.setting.*.read"),
    #[cfg(feature = "permitted-flags")] permitted("permitted_routes", "POST", "/settings", "change_setting", "sofie_identity.setting.*.update"),
    #[cfg(feature = "permitted-shield")] permitted("permitted_routes", "GET", "/shield", "list_blocks", "sofie_identity.shield.*.read"),
    #[cfg(feature = "permitted-shield")] permitted("permitted_routes", "POST", "/shield", "lift_block", "sofie_identity.shield.*.delete"),
    #[cfg(feature = "token-screens")] permitted("tokens::screen::routes", "GET", "/tokens", "list_tokens", "sofie_identity.access_token.*.read"),
    #[cfg(feature = "token-screens")] permitted("tokens::screen::routes", "POST", "/tokens", "revoke_token", "sofie_identity.access_token.*.delete"),
    #[cfg(feature = "sessions")] ListedRoute::new("sessions::routes", "GET", "/sessions", "list_sessions"),
    #[cfg(feature = "sessions")] ListedRoute::new("sessions::routes", "POST", "/sessions/{account_session}/end", "end_session"),
    #[cfg(feature = "sessions")] ListedRoute::new("sessions::routes", "POST", "/sessions/others", "end_other_sessions"),
    #[cfg(feature = "totp")] ListedRoute::new("two_factor::routes", "GET", "/two-factor", "two_factor_page"),
    #[cfg(feature = "totp")] ListedRoute::new("two_factor::routes", "POST", "/two-factor/start", "start_two_factor"),
    #[cfg(feature = "totp")] ListedRoute::new("two_factor::routes", "POST", "/two-factor/confirm", "confirm_two_factor"),
    #[cfg(feature = "totp")] ListedRoute::new("two_factor::routes", "POST", "/two-factor/disable", "disable_two_factor"),
    #[cfg(feature = "passkeys")] ListedRoute::new("passkeys::routes", "GET", "/passkeys", "list_passkeys"),
    #[cfg(feature = "passkeys")] ListedRoute::new("passkeys::routes", "POST", "/passkeys", "register_passkey"),
    #[cfg(feature = "passkeys")] ListedRoute::new("passkeys::routes", "POST", "/passkeys/{passkey}/remove", "remove_passkey"),
    #[cfg(feature = "oauth")] ListedRoute::new("oauth::routes", "GET", "/connections", "list_connections"),
    #[cfg(feature = "oauth")] ListedRoute::new("oauth::routes", "POST", "/connections", "connect_provider"),
    #[cfg(feature = "oauth")] ListedRoute::new("oauth::routes", "POST", "/connections/{account_identity}/remove", "disconnect_provider"),
    #[cfg(feature = "account-data")] ListedRoute::new("account_data::routes", "GET", "/account-data", "account_data_page"),
    #[cfg(feature = "account-data")] ListedRoute::new("account_data::routes", "POST", "/account-data/export", "export_account_data"),
    #[cfg(feature = "account-data")] ListedRoute::new("account_data::routes", "POST", "/account-data/close", "close_account"),
    #[cfg(feature = "impersonation")] permitted("impersonation::routes", "GET", "/impersonation", "list_impersonations", "sofie_identity.impersonation.*.read"),
    #[cfg(feature = "impersonation")] permitted("impersonation::routes", "POST", "/impersonation", "impersonate", "sofie_identity.impersonation.*.create"),
    #[cfg(feature = "impersonation")] ListedRoute::new("impersonation::stop_routes", "POST", "/impersonation/stop", "stop_impersonating"),
];
