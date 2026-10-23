-- A link mailed to sign in without a password.
ALTER TABLE account_link DROP CONSTRAINT account_link_purpose_check;
ALTER TABLE account_link ADD CONSTRAINT account_link_purpose_check
    CHECK (purpose IN ('PASSWORD_RESET', 'EMAIL_VERIFICATION', 'SIGN_IN'));
