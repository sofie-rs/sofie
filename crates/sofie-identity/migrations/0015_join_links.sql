-- A link mailed to an account signing up at an area it is no member of, to sign in and join.
ALTER TABLE account_link DROP CONSTRAINT account_link_purpose_check;
ALTER TABLE account_link ADD CONSTRAINT account_link_purpose_check
    CHECK (purpose IN ('PASSWORD_RESET', 'EMAIL_VERIFICATION', 'SIGN_IN', 'SECOND_FACTOR', 'JOIN'));
