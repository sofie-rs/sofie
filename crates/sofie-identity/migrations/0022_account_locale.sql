-- The language the account chose for its pages and mails; null follows the browser's.
ALTER TABLE account ADD COLUMN locale text;
