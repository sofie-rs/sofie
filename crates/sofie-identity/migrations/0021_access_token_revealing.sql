-- A new token's text, sealed, until its owner has seen it on the token screen.
ALTER TABLE access_token ADD COLUMN revealing text;
