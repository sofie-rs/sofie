# The two apps compared, sourced by run.sh and limits.sh after they set
# BENCH and ROOT. `setting <app> NAME` in measure.sh reads these.

# ---- The framework's shop (examples/shop) -------------------------------------
SOFIE_NAME="framework"
SOFIE_DIR="$ROOT/examples/shop"
SOFIE_PORT=${SOFIE_PORT:-8080}
SOFIE_READY=/sign-in
SOFIE_SIGN_UP=/sign-up
SOFIE_SIGN_IN=/sign-in
SOFIE_SESSION_COOKIE=__Host-shop-session
SOFIE_LIST_JSON=/account/orders
SOFIE_PAGE=/account/orders
SOFIE_PLACE=/account/orders
SOFIE_PLACE_FORM='total.amount=12.50&total.currency=EUR'
SOFIE_HANDLER="$SOFIE_DIR/modules/order/src/actions.rs"
SOFIE_BINARY=shop
SOFIE_SEED_SQL="
INSERT INTO \"order\".\"order\"
    (id, version, email, status, total, total_currency, placed_at, account,
     created_at, updated_at)
SELECT uuidv7(), 1, 'buyer' || n || '@example.com',
       (ARRAY['PENDING', 'PAID', 'SHIPPED', 'CANCELLED'])[1 + n % 4],
       1000 + n, 'EUR', now() - n * interval '1 second', account.id, now(), now()
FROM sofie_identity.account, generate_series(1, :orders) AS n;"

# ---- The Loco twin (bench/loco-shop) ------------------------------------------
LOCO_NAME="Loco"
LOCO_DIR="$BENCH/loco-shop"
LOCO_PORT=5150
LOCO_READY=/sign-in
LOCO_SIGN_UP=/sign-up
LOCO_SIGN_IN=/sign-in
LOCO_SESSION_COOKIE=auth_token
LOCO_LIST_JSON=/api/orders
LOCO_PAGE=/orders
LOCO_PLACE=/orders
LOCO_PLACE_FORM='email=buyer%40example.com&amount=12.50&currency=EUR'
LOCO_HANDLER="$LOCO_DIR/src/controllers/orders.rs"
LOCO_BINARY=loco_shop-cli
LOCO_SEED_SQL="
INSERT INTO orders (email, status, total, currency, placed_at, user_id)
SELECT 'buyer' || n || '@example.com',
       (ARRAY['PENDING', 'PAID', 'SHIPPED', 'CANCELLED'])[1 + n % 4],
       1000 + n, 'EUR', now() - n * interval '1 second', users.id
FROM users, generate_series(1, :orders) AS n;"
