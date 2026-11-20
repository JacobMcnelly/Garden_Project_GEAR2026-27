//! Beds, plantings and the admin check (ADR 0003).

use std::sync::PoisonError;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{Connection, Row, ffi, named_params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{ApiError, App};

fn err(code: StatusCode, msg: &str) -> ApiError {
    (code, Json(json!({ "error": msg })))
}

/// `GARDEN_ADMINS`: Tailscale logins separated by commas.
pub fn parse_admins(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect()
}

/// Set by `tailscale serve`, which drops any copy a client sends (ADR 0003). No header means a viewer.
fn login(headers: &HeaderMap) -> Option<&str> {
    headers.get("tailscale-user-login")?.to_str().ok()
}

/// The admin's login, or `403` for a viewer.
fn admin(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    login(headers)
        .filter(|l| app.admins.iter().any(|a| a.eq_ignore_ascii_case(l)))
        .map(String::from)
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "admins only"))
}

pub async fn get_me(State(app): State<App>, headers: HeaderMap) -> Json<Value> {
    let admin = admin(&app, &headers).is_ok();
    Json(json!({ "login": login(&headers), "admin": admin }))
}

/// True for a real calendar date written as `YYYY-MM-DD`.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !digits(0..4) || !digits(5..7) || !digits(8..10) {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap();
    let (y, m, d) = (num(0..4), num(5..7), num(8..10));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return false,
    };
    (1..=days).contains(&d)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Bed {
    #[serde(skip_deserializing)]
    id: i64,
    name: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Planting {
    #[serde(skip_deserializing)]
    id: i64,
    bed_id: i64,
    plant: String,
    #[serde(default)]
    variety: String,
    planted_on: String,
    removed_on: Option<String>,
    #[serde(default)]
    notes: String,
    #[serde(skip_deserializing)]
    changed_by: String,
    #[serde(skip_deserializing)]
    changed_at: i64,
}

fn bed_row(r: &Row) -> rusqlite::Result<Bed> {
    Ok(Bed {
        id: r.get("id")?,
        name: r.get("name")?,
        location: r.get("location")?,
        notes: r.get("notes")?,
    })
}

fn planting_row(r: &Row) -> rusqlite::Result<Planting> {
    Ok(Planting {
        id: r.get("id")?,
        bed_id: r.get("bed_id")?,
        plant: r.get("plant")?,
        variety: r.get("variety")?,
        planted_on: r.get("planted_on")?,
        removed_on: r.get("removed_on")?,
        notes: r.get("notes")?,
        changed_by: r.get("changed_by")?,
        changed_at: r.get("changed_at")?,
    })
}

fn check_bed(b: &mut Bed) -> Result<(), ApiError> {
    b.name = b.name.trim().into();
    if b.name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name must not be empty"));
    }
    Ok(())
}

fn check_planting(p: &mut Planting) -> Result<(), ApiError> {
    p.plant = p.plant.trim().into();
    let bad = |msg| Err(err(StatusCode::BAD_REQUEST, msg));
    if p.plant.is_empty() {
        return bad("plant must not be empty");
    }
    if !is_date(&p.planted_on) {
        return bad("planted_on must be a real date like 2026-04-15");
    }
    match &p.removed_on {
        Some(r) if !is_date(r) => bad("removed_on must be a real date like 2026-04-15"),
        // YYYY-MM-DD strings sort like the dates they name.
        Some(r) if *r < p.planted_on => bad("removed_on must not be before planted_on"),
        _ => Ok(()),
    }
}

/// Reads a JSON body into a row. Fields only the server sets (id, changed_by, changed_at) are ignored.
fn parse<T: DeserializeOwned>(body: Value) -> Result<T, ApiError> {
    serde_json::from_value(body).map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))
}

/// For PATCH: the saved row with the sent fields copied over it, so a sent `null` clears a field.
fn merge<T: Serialize + DeserializeOwned>(
    saved: &T,
    body: Map<String, Value>,
) -> Result<T, ApiError> {
    let mut row = serde_json::to_value(saved).unwrap();
    row.as_object_mut().unwrap().extend(body);
    parse(row)
}

fn is_foreign_key(e: &rusqlite::Error) -> bool {
    e.sqlite_error()
        .is_some_and(|f| f.extended_code == ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
}

fn db_error(e: rusqlite::Error) -> ApiError {
    if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    if e.sqlite_error()
        .is_some_and(|f| f.extended_code == ffi::SQLITE_CONSTRAINT_UNIQUE)
    {
        return err(StatusCode::CONFLICT, "a bed with that name already exists");
    }
    // Only planting writes reach this; delete_bed handles its own foreign key error.
    if is_foreign_key(&e) {
        return err(StatusCode::BAD_REQUEST, "bed_id is not an existing bed");
    }
    eprintln!("database: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

/// Runs `f` with the database on a blocking thread.
async fn with_db<T: Send + 'static>(
    app: &App,
    f: impl FnOnce(&Connection) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let db = app.db.clone();
    tokio::task::spawn_blocking(move || f(&db.lock().unwrap_or_else(PoisonError::into_inner)))
        .await
        .unwrap_or_else(|e| {
            eprintln!("database: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub async fn get_beds(State(app): State<App>) -> Result<Json<Vec<Bed>>, ApiError> {
    with_db(&app, |c| {
        let mut stmt = c
            .prepare_cached(include_str!("../../db/list_beds.sql"))
            .map_err(db_error)?;
        let rows = stmt.query_map([], bed_row).map_err(db_error)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_error)
    })
    .await
    .map(Json)
}

pub async fn post_bed(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Bed>), ApiError> {
    admin(&app, &headers)?;
    let mut b: Bed = parse(body)?;
    check_bed(&mut b)?;
    let bed = with_db(&app, move |c| {
        let mut stmt = c
            .prepare_cached(include_str!("../../db/insert_bed.sql"))
            .map_err(db_error)?;
        stmt.query_row(
            named_params! {":name": b.name, ":location": b.location, ":notes": b.notes},
            bed_row,
        )
        .map_err(db_error)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(bed)))
}

pub async fn patch_bed(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<Map<String, Value>>,
) -> Result<Json<Bed>, ApiError> {
    admin(&app, &headers)?;
    with_db(&app, move |c| {
        let saved = c
            .prepare_cached(include_str!("../../db/find_bed.sql"))
            .and_then(|mut s| s.query_row(named_params! {":id": id}, bed_row))
            .map_err(db_error)?;
        let mut b: Bed = merge(&saved, body)?;
        check_bed(&mut b)?;
        let mut stmt = c
            .prepare_cached(include_str!("../../db/update_bed.sql"))
            .map_err(db_error)?;
        stmt.query_row(
            named_params! {":id": id, ":name": b.name, ":location": b.location, ":notes": b.notes},
            bed_row,
        )
        .map_err(db_error)
    })
    .await
    .map(Json)
}

pub async fn delete_bed(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    admin(&app, &headers)?;
    with_db(&app, move |c| {
        match c.execute(
            include_str!("../../db/delete_bed.sql"),
            named_params! {":id": id},
        ) {
            Ok(0) => Err(err(StatusCode::NOT_FOUND, "not found")),
            Ok(_) => Ok(StatusCode::NO_CONTENT),
            Err(e) if is_foreign_key(&e) => {
                Err(err(StatusCode::CONFLICT, "bed still has plantings"))
            }
            Err(e) => Err(db_error(e)),
        }
    })
    .await
}

#[derive(Deserialize)]
pub struct PlantingsParams {
    #[serde(default)]
    current: u8,
}

pub async fn get_plantings(
    State(app): State<App>,
    Query(p): Query<PlantingsParams>,
) -> Result<Json<Vec<Planting>>, ApiError> {
    with_db(&app, move |c| {
        let mut stmt = c
            .prepare_cached(include_str!("../../db/list_plantings.sql"))
            .map_err(db_error)?;
        let rows = stmt
            .query_map(named_params! {":current": p.current != 0}, planting_row)
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_error)
    })
    .await
    .map(Json)
}

/// Inserts `p` when `id` is `None`, otherwise saves it over that planting.
fn save_planting(c: &Connection, id: Option<i64>, p: &Planting) -> Result<Planting, ApiError> {
    let sql = match id {
        None => include_str!("../../db/insert_planting.sql"),
        Some(_) => include_str!("../../db/update_planting.sql"),
    };
    let mut params = named_params! {
        ":bed_id": p.bed_id, ":plant": p.plant, ":variety": p.variety,
        ":planted_on": p.planted_on, ":removed_on": p.removed_on, ":notes": p.notes,
        ":changed_by": p.changed_by, ":changed_at": p.changed_at,
    }
    .to_vec();
    if let Some(id) = &id {
        params.push((":id", id));
    }
    let mut stmt = c.prepare_cached(sql).map_err(db_error)?;
    stmt.query_row(params.as_slice(), planting_row)
        .map_err(db_error)
}

pub async fn post_planting(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Planting>), ApiError> {
    let login = admin(&app, &headers)?;
    let mut p: Planting = parse(body)?;
    check_planting(&mut p)?;
    (p.changed_by, p.changed_at) = (login, unix_now());
    let p = with_db(&app, move |c| save_planting(c, None, &p)).await?;
    Ok((StatusCode::CREATED, Json(p)))
}

pub async fn patch_planting(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<Map<String, Value>>,
) -> Result<Json<Planting>, ApiError> {
    let login = admin(&app, &headers)?;
    with_db(&app, move |c| {
        let saved = c
            .prepare_cached(include_str!("../../db/find_planting.sql"))
            .and_then(|mut s| s.query_row(named_params! {":id": id}, planting_row))
            .map_err(db_error)?;
        let mut p: Planting = merge(&saved, body)?;
        check_planting(&mut p)?;
        (p.changed_by, p.changed_at) = (login, unix_now());
        save_planting(c, Some(id), &p)
    })
    .await
    .map(Json)
}

pub async fn delete_planting(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    admin(&app, &headers)?;
    with_db(&app, move |c| {
        match c.execute(
            include_str!("../../db/delete_planting.sql"),
            named_params! {":id": id},
        ) {
            Ok(0) => Err(err(StatusCode::NOT_FOUND, "not found")),
            Ok(_) => Ok(StatusCode::NO_CONTENT),
            Err(e) => Err(db_error(e)),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;
    use std::sync::{Arc, Mutex};

    fn app() -> App {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        migrate(&mut conn).unwrap();
        App {
            db: Arc::new(Mutex::new(conn)),
            sensors: Default::default(),
            admins: Arc::new(parse_admins(" sam@example.com, ,alex@example.com,")),
        }
    }

    fn as_user(login: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("tailscale-user-login", login.parse().unwrap());
        h
    }

    fn sam() -> HeaderMap {
        as_user("sam@example.com")
    }

    async fn new_bed(app: &App, name: &str) -> Bed {
        post_bed(State(app.clone()), sam(), Json(json!({"name": name})))
            .await
            .unwrap()
            .1
            .0
    }

    async fn new_planting(app: &App, body: Value) -> Result<Planting, StatusCode> {
        post_planting(State(app.clone()), sam(), Json(body))
            .await
            .map(|(_, Json(p))| p)
            .map_err(|e| e.0)
    }

    async fn plantings(app: &App, current: u8) -> Vec<i64> {
        let Json(list) = get_plantings(State(app.clone()), Query(PlantingsParams { current }))
            .await
            .unwrap();
        list.iter().map(|p| p.id).collect()
    }

    #[test]
    fn admins_list_is_trimmed() {
        assert_eq!(
            *app().admins,
            vec!["sam@example.com".to_string(), "alex@example.com".into()]
        );
        assert!(parse_admins("").is_empty());
    }

    #[tokio::test]
    async fn admin_check_uses_the_tailscale_header() {
        let app = app();
        assert_eq!(admin(&app, &sam()).unwrap(), "sam@example.com");
        assert!(admin(&app, &as_user("Sam@Example.com")).is_ok()); // emails ignore case
        assert_eq!(
            admin(&app, &as_user("eve@example.com")).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        assert!(admin(&app, &HeaderMap::new()).is_err());
        let Json(me) = get_me(State(app.clone()), sam()).await;
        assert_eq!(me, json!({"login": "sam@example.com", "admin": true}));
        let Json(me) = get_me(State(app), HeaderMap::new()).await;
        assert_eq!(me, json!({"login": null, "admin": false}));
    }

    #[test]
    fn dates_must_be_real() {
        for ok in ["2026-04-15", "2028-02-29", "2000-02-29", "2026-12-31"] {
            assert!(is_date(ok), "{ok}");
        }
        for bad in [
            "2026-02-30",
            "2026-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-04-00",
            "2026-4-15",
            "2026-04-15 ",
            "+026-04-15",
            "15/04/2026",
            "",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn viewer_cannot_write() {
        let app = app();
        let bed = new_bed(&app, "Bed 1").await;
        for viewer in [HeaderMap::new(), as_user("eve@example.com")] {
            let res = post_bed(
                State(app.clone()),
                viewer.clone(),
                Json(json!({"name": "X"})),
            )
            .await;
            assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
            let res = delete_bed(State(app.clone()), viewer, Path(bed.id)).await;
            assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn bed_round_trip() {
        let app = app();
        let bed = new_bed(&app, "  Bed 3 ").await;
        assert_eq!(
            serde_json::to_value(&bed).unwrap(),
            json!({"id": bed.id, "name": "Bed 3", "location": "", "notes": ""})
        );
        let Json(b) = patch_bed(
            State(app.clone()),
            sam(),
            Path(bed.id),
            Json(
                json!({"notes": "sunny", "id": 99})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            (b.id, b.name.as_str(), b.notes.as_str()),
            (bed.id, "Bed 3", "sunny")
        );
        let Json(all) = get_beds(State(app.clone())).await.unwrap();
        assert_eq!(all.len(), 1);
        let del = |id| delete_bed(State(app.clone()), sam(), Path(id));
        assert_eq!(del(bed.id).await.unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(del(bed.id).await.unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bed_name_must_be_unique_and_not_empty() {
        let app = app();
        new_bed(&app, "Bed 1").await;
        let post = |name: &str| post_bed(State(app.clone()), sam(), Json(json!({"name": name})));
        assert_eq!(post("Bed 1").await.unwrap_err().0, StatusCode::CONFLICT);
        assert_eq!(post("  ").await.unwrap_err().0, StatusCode::BAD_REQUEST);
        let res = patch_bed(State(app.clone()), sam(), Path(42), Json(Map::new())).await;
        assert_eq!(res.unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn planting_round_trip() {
        let app = app();
        let bed = new_bed(&app, "Bed 1").await;
        let p = new_planting(
            &app,
            json!({"bed_id": bed.id, "plant": "Tomato", "variety": "Roma", "planted_on": "2026-04-15"}),
        )
        .await
        .unwrap();
        assert_eq!(p.changed_by, "sam@example.com");
        assert!(p.changed_at > 0);
        assert_eq!(p.removed_on, None);

        let patch = |body: Value| {
            patch_planting(
                State(app.clone()),
                as_user("alex@example.com"),
                Path(p.id),
                Json(body.as_object().unwrap().clone()),
            )
        };
        let Json(q) = patch(json!({"removed_on": "2026-09-01"})).await.unwrap();
        assert_eq!(q.removed_on.as_deref(), Some("2026-09-01"));
        assert_eq!(
            (q.plant.as_str(), q.changed_by.as_str()),
            ("Tomato", "alex@example.com")
        );
        assert!(plantings(&app, 1).await.is_empty());
        assert_eq!(plantings(&app, 0).await, vec![p.id]);

        let Json(q) = patch(json!({"notes": "oops"})).await.unwrap();
        assert_eq!(q.removed_on.as_deref(), Some("2026-09-01")); // absent: unchanged
        let Json(q) = patch(json!({"removed_on": null})).await.unwrap();
        assert_eq!(q.removed_on, None); // null: cleared
        assert_eq!(plantings(&app, 1).await, vec![p.id]);

        let del = |id| delete_planting(State(app.clone()), sam(), Path(id));
        assert_eq!(del(p.id).await.unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(del(p.id).await.unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn planting_input_is_checked() {
        let app = app();
        let bed = new_bed(&app, "Bed 1").await;
        for body in [
            json!({"bed_id": bed.id, "plant": "Tomato", "planted_on": "2026-02-30"}),
            json!({"bed_id": bed.id, "plant": " ", "planted_on": "2026-04-15"}),
            json!({"bed_id": bed.id, "plant": "Tomato", "planted_on": "2026-04-15", "removed_on": "2026-04-14"}),
            json!({"bed_id": bed.id + 1, "plant": "Tomato", "planted_on": "2026-04-15"}),
            json!({"bed_id": bed.id, "plant": "Tomato"}),
        ] {
            assert_eq!(
                new_planting(&app, body.clone()).await.unwrap_err(),
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn bed_with_plantings_cannot_be_deleted() {
        let app = app();
        let bed = new_bed(&app, "Bed 1").await;
        new_planting(
            &app,
            json!({"bed_id": bed.id, "plant": "Kale", "planted_on": "2026-04-15"}),
        )
        .await
        .unwrap();
        let (code, Json(body)) = delete_bed(State(app), sam(), Path(bed.id))
            .await
            .unwrap_err();
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body, json!({"error": "bed still has plantings"}));
    }
}
