-- Named after what it holds, the payment provider's own id of the subscriber.
ALTER TABLE subscription RENAME COLUMN customer TO provider_customer;
