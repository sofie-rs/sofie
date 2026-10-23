use axum::http::StatusCode;
use loco_rs::prelude::*;

use crate::models::users::{self, LoginParams, RegisterParams};

const SESSION_COOKIE: &str = "auth_token";

fn form(v: &TeraView, template: &str, error: Option<&str>) -> Result<Response> {
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    format::render()
        .status(status)
        .view(v, template, data!({ "error": error }))
}

/// Signs the user in by setting the JWT cookie the `auth` extractors read.
fn signed_in(ctx: &AppContext, user: &users::Model) -> Result<Response> {
    let jwt = ctx.config.get_jwt_config()?;
    let token = user
        .generate_jwt(&jwt.secret, jwt.expiration)
        .or_else(|_| unauthorized("unauthorized!"))?;
    let cookie = cookie::Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .build();
    format::render().cookies(&[cookie])?.redirect("/orders")
}

#[debug_handler]
async fn sign_up_form(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    form(&v, "sessions/sign_up.html", None)
}

#[debug_handler]
async fn sign_up(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Form(params): Form<RegisterParams>,
) -> Result<Response> {
    match users::Model::create_with_password(&ctx.db, &params).await {
        Ok(user) => signed_in(&ctx, &user),
        Err(ModelError::EntityAlreadyExists) => form(
            &v,
            "sessions/sign_up.html",
            Some("That email is already registered."),
        ),
        // The users model reports failed validation as a custom database error.
        Err(ModelError::DbErr(DbErr::Custom(_))) => form(
            &v,
            "sessions/sign_up.html",
            Some("Check your name and email."),
        ),
        Err(error) => Err(error.into()),
    }
}

#[debug_handler]
async fn sign_in_form(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    form(&v, "sessions/sign_in.html", None)
}

#[debug_handler]
async fn sign_in(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Form(params): Form<LoginParams>,
) -> Result<Response> {
    match users::Model::find_by_email(&ctx.db, &params.email).await {
        Ok(user) if user.verify_password(&params.password) => signed_in(&ctx, &user),
        Ok(_) | Err(ModelError::EntityNotFound) => form(
            &v,
            "sessions/sign_in.html",
            Some("Wrong email or password."),
        ),
        Err(error) => Err(error.into()),
    }
}

#[debug_handler]
async fn sign_out() -> Result<Response> {
    let cookie = cookie::Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .removal()
        .build();
    format::render().cookies(&[cookie])?.redirect("/sign-in")
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/sign-up", get(sign_up_form))
        .add("/sign-up", post(sign_up))
        .add("/sign-in", get(sign_in_form))
        .add("/sign-in", post(sign_in))
        .add("/sign-out", post(sign_out))
}
