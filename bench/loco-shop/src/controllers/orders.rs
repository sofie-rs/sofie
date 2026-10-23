use axum::http::StatusCode;
use loco_rs::prelude::*;

use crate::{
    models::{
        orders::{self, PlaceParams},
        users,
    },
    views::orders::OrderResponse,
    workers::audit::{Worker as AuditWorker, WorkerArgs as AuditArgs},
};

type SignedIn = auth::JWTWithUser<users::Model>;

async fn audit(ctx: &AppContext, order: &orders::Model, action: &str) -> Result<()> {
    let args = AuditArgs {
        order_id: order.id,
        action: action.to_owned(),
    };
    AuditWorker::perform_later(ctx, args).await?;
    Ok(())
}

async fn place_order(
    ctx: &AppContext,
    user: &users::Model,
    params: &PlaceParams,
) -> ModelResult<orders::Model> {
    let order = orders::Model::place(&ctx.db, user.id, params).await?;
    audit(ctx, &order, "PLACED")
        .await
        .map_err(ModelError::to_msg)?;
    Ok(order)
}

async fn cancel_order(
    ctx: &AppContext,
    user: &users::Model,
    id: i64,
) -> ModelResult<orders::Model> {
    let order = orders::Model::find_for_user(&ctx.db, user.id, id).await?;
    let order = order.into_active_model().cancel(&ctx.db).await?;
    audit(ctx, &order, "CANCELLED")
        .await
        .map_err(ModelError::to_msg)?;
    Ok(order)
}

async fn list_orders(ctx: &AppContext, user: &users::Model) -> Result<Vec<OrderResponse>> {
    let orders = orders::Model::list_for_user(&ctx.db, user.id).await?;
    Ok(orders.iter().map(OrderResponse::from).collect())
}

/// The orders page, with `errors` shown above the form.
async fn render_page(
    v: &TeraView,
    ctx: &AppContext,
    user: &users::Model,
    errors: &[String],
) -> Result<Response> {
    let orders = list_orders(ctx, user).await?;
    let status = if errors.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    format::render().status(status).view(
        v,
        "orders/index.html",
        data!({ "user": user.name, "orders": orders, "errors": errors }),
    )
}

/// After a form post: back to the page, or the page again with what went wrong.
async fn answer_form(
    outcome: ModelResult<orders::Model>,
    v: &TeraView,
    ctx: &AppContext,
    user: &users::Model,
) -> Result<Response> {
    match outcome {
        Ok(_) => format::redirect("/orders"),
        Err(ModelError::Validation(invalid)) => {
            let errors: Vec<String> = invalid
                .errors
                .iter()
                .flat_map(|(field, errors)| {
                    errors.iter().map(move |error| {
                        format!(
                            "{field}: {}",
                            error.message.as_deref().unwrap_or(&error.code)
                        )
                    })
                })
                .collect();
            render_page(v, ctx, user, &errors).await
        }
        Err(error) => Err(error.into()),
    }
}

#[debug_handler]
async fn page(
    auth: SignedIn,
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
) -> Result<Response> {
    render_page(&v, &ctx, &auth.user, &[]).await
}

#[debug_handler]
async fn place_from_form(
    auth: SignedIn,
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Form(params): Form<PlaceParams>,
) -> Result<Response> {
    let outcome = place_order(&ctx, &auth.user, &params).await;
    answer_form(outcome, &v, &ctx, &auth.user).await
}

#[debug_handler]
async fn cancel_from_form(
    auth: SignedIn,
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let outcome = cancel_order(&ctx, &auth.user, id).await;
    answer_form(outcome, &v, &ctx, &auth.user).await
}

#[debug_handler]
async fn list(auth: SignedIn, State(ctx): State<AppContext>) -> Result<Response> {
    format::json(list_orders(&ctx, &auth.user).await?)
}

#[debug_handler]
async fn place(
    auth: SignedIn,
    State(ctx): State<AppContext>,
    Json(params): Json<PlaceParams>,
) -> Result<Response> {
    let order = place_order(&ctx, &auth.user, &params).await?;
    format::json(OrderResponse::from(&order))
}

#[debug_handler]
async fn cancel(
    auth: SignedIn,
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let order = cancel_order(&ctx, &auth.user, id).await?;
    format::json(OrderResponse::from(&order))
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/orders", get(page))
        .add("/orders", post(place_from_form))
        .add("/orders/{id}/cancel", post(cancel_from_form))
        .add("/api/orders", get(list))
        .add("/api/orders", post(place))
        .add("/api/orders/{id}/cancel", post(cancel))
}
