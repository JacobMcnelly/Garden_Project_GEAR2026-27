use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{Connection, Row, ffi, named_params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tower_http::services::ServeDir;

const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../../db/migrations/0001_init.sql")),
    (2, include_str!("../../db/migrations/0002_beds.sql")),
];

fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // WAL can't be switched on inside a transaction, so pragmas live here, not in a migration.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
    )?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for &(version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[derive(Serialize)]
struct History {
    bucket: i64,
    t: Vec<i64>,
    sensors: Vec<SensorSeries>,
}

#[derive(Serialize)]
struct SensorSeries {
    name: String,
    unit: Option<String>,
    values: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct HistoryParams {
    from: i64,
    to: i64,
}

/// `max(60, ceil(span / 1000))`, so `t` never has more than 1001 entries. `None` if `to <= from`.
fn bucket_size(from: i64, to: i64) -> Option<i64> {
    let span = to.checked_sub(from).filter(|s| *s > 0)?;
    Some(((span - 1) / 1000 + 1).max(60))
}

fn history(conn: &Connection, from: i64, to: i64, bucket: i64) -> rusqlite::Result<History> {
    let start = from / bucket * bucket;
    let n = (to / bucket * bucket - start) / bucket + 1;
    let t: Vec<i64> = (0..n).map(|i| start + i * bucket).collect();

    let mut sensors: Vec<SensorSeries> = Vec::new();
    let mut stmt = conn.prepare_cached(include_str!("../../db/history.sql"))?;
    let mut rows = stmt.query(named_params! {":from": from, ":to": to, ":bucket": bucket})?;
    while let Some(row) = rows.next()? {
        let name: String = row.get("name")?;
        if sensors.last().is_none_or(|s| s.name != name) {
            sensors.push(SensorSeries {
                name,
                unit: row.get("unit")?,
                values: vec![None; t.len()],
            });
        }
        let i = (row.get::<_, i64>("t")? - start) / bucket;
        sensors.last_mut().unwrap().values[i as usize] = row.get("avg_value")?;
    }
    Ok(History { bucket, t, sensors })
}

type ApiError = (StatusCode, Json<Value>);
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
struct App {
    db: Arc<Mutex<Connection>>,
    sensors: Arc<tokio::sync::Mutex<Sensors>>,
    admins: Arc<Vec<String>>,
}

async fn get_history(
    State(app): State<App>,
    Query(p): Query<HistoryParams>,
) -> Result<Json<History>, ApiError> {
    let conn = app.db;
    let bucket = bucket_size(p.from, p.to).ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "to must be greater than from"})),
    ))?;
    let res = tokio::task::spawn_blocking(move || {
        // A panic elsewhere must not lock every later request out of the database.
        let conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
        history(&conn, p.from, p.to, bucket)
    })
    .await;
    let err = match res {
        Ok(Ok(h)) => return Ok(Json(h)),
        Ok(Err(e)) => e.to_string(),
        Err(e) => e.to_string(),
    };
    eprintln!("/api/history: {err}");
    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    ))
}

// The venv's python directly, not `uv run`: killing a `uv` wrapper can leave a hung python holding the sensors.
const DEFAULT_SENSORS_CMD: &str = "sensors/.venv/bin/python sensors/sensors.py";

/// The running `sensors.py` child.
struct Python {
    _child: Child, // kill_on_drop: dropping this stops Python
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Python {
    fn spawn() -> Result<Python, Error> {
        let cmd = std::env::var("GARDEN_SENSORS_CMD").unwrap_or(DEFAULT_SENSORS_CMD.into());
        // ponytail: split on whitespace, no shell, so quoted args with spaces aren't supported.
        let mut words = cmd.split_whitespace();
        let program = words.next().ok_or("GARDEN_SENSORS_CMD is empty")?;
        let mut child = tokio::process::Command::new(program)
            .args(words)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        eprintln!("sensors: started `{cmd}`");
        Ok(Python {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()).lines(),
            _child: child,
        })
    }

    /// Sends one command and waits up to 5 s for the first line that is a JSON object.
    async fn request(&mut self, cmd: &str) -> Result<Map<String, Value>, Error> {
        let exchange = async {
            let msg = format!("{}\n", json!({ "cmd": cmd }));
            self.stdin.write_all(msg.as_bytes()).await?;
            loop {
                let line = self.stdout.next_line().await?.ok_or("Python exited")?;
                match json_object(&line) {
                    Some(reply) => return Ok(reply),
                    None => eprintln!("sensors: skipped non-JSON line: {line}"),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), exchange)
            .await
            .map_err(|_| format!("no reply to {cmd} within 5 s"))?
    }
}

/// `None` = skip this line (e.g. a beginner's debug `print()`).
fn json_object(line: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str(line) {
        Ok(Value::Object(reply)) => Some(reply),
        _ => None,
    }
}

/// `None` unless every unit is a string.
fn describe_units(reply: Map<String, Value>) -> Option<BTreeMap<String, String>> {
    reply
        .into_iter()
        .map(|(name, unit)| match unit {
            Value::String(unit) => Some((name, unit)),
            _ => None,
        })
        .collect()
}

/// Keeps only numbers for described sensors; `null` (sensor failed) is simply absent.
fn read_values(
    reply: Map<String, Value>,
    units: &BTreeMap<String, String>,
) -> BTreeMap<String, f64> {
    let mut values = BTreeMap::new();
    for (name, value) in reply {
        match value.as_f64() {
            _ if !units.contains_key(&name) => {
                eprintln!("sensors: dropped unknown sensor {name:?}")
            }
            Some(v) => {
                values.insert(name, v);
            }
            None if value.is_null() => {}
            None => eprintln!("sensors: dropped non-number {name:?}: {value}"),
        }
    }
    values
}

fn upsert_sensors(conn: &Connection, units: &BTreeMap<String, String>) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(include_str!("../../db/upsert_sensor.sql"))?;
    for (name, unit) in units {
        stmt.execute(named_params! {":name": name, ":unit": unit})?;
    }
    Ok(())
}

/// Saves one snapshot in a single transaction: all of it or none.
fn save_snapshot(
    conn: &mut Connection,
    ts: i64,
    values: &BTreeMap<String, f64>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(include_str!("../../db/insert_reading.sql"))?;
        for (name, value) in values {
            stmt.execute(named_params! {":ts": ts, ":name": name, ":value": value})?;
        }
    }
    tx.commit()
}

/// Time until the next wall-clock minute; a full minute at :00 exactly, so one minute never gets two snapshots.
fn until_next_minute(since_epoch: Duration) -> Duration {
    Duration::from_millis(60_000 - (since_epoch.as_millis() % 60_000) as u64)
}

/// Saves a snapshot at the start of every minute. A late or missed minute is skipped, never caught up.
async fn record_every_minute(app: App) {
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        tokio::time::sleep(until_next_minute(now)).await;
        // Held through the save, so shutdown (which takes this lock) waits for it.
        let mut sensors = app.sensors.lock().await;
        let Some((ts, values)) = sensors.snapshot(&app.db).await else {
            eprintln!("recorder: no snapshot this minute");
            continue;
        };
        let db = app.db.clone();
        let res = tokio::task::spawn_blocking(move || {
            save_snapshot(
                &mut db.lock().unwrap_or_else(PoisonError::into_inner),
                ts,
                &values,
            )
        })
        .await;
        let err = match res {
            Ok(Ok(())) => continue,
            Ok(Err(e)) => e.to_string(),
            Err(e) => e.to_string(),
        };
        eprintln!("recorder: save failed: {err}");
    }
}

#[derive(Default)]
struct Sensors {
    python: Option<Python>,
    units: BTreeMap<String, String>, // from the latest describe
}

impl Sensors {
    /// One snapshot as `(ts, values)`, or `None` if Python failed (it's stopped, and restarted on the next call).
    async fn snapshot(
        &mut self,
        db: &Arc<Mutex<Connection>>,
    ) -> Option<(i64, BTreeMap<String, f64>)> {
        let res = self.try_snapshot(db).await;
        if let Err(e) = &res {
            eprintln!("sensors unavailable, stopping Python: {e}");
            self.python = None;
        }
        res.ok()
    }

    async fn try_snapshot(
        &mut self,
        db: &Arc<Mutex<Connection>>,
    ) -> Result<(i64, BTreeMap<String, f64>), Error> {
        if self.python.is_none() {
            let mut python = Python::spawn()?;
            let units = describe_units(python.request("describe").await?)
                .ok_or("describe reply must map each name to a unit string")?;
            let (db, u) = (db.clone(), units.clone());
            tokio::task::spawn_blocking(move || {
                upsert_sensors(&db.lock().unwrap_or_else(PoisonError::into_inner), &u)
            })
            .await??;
            self.units = units;
            self.python = Some(python);
        }
        let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let reply = self.python.as_mut().unwrap().request("read").await?;
        Ok((ts, read_values(reply, &self.units)))
    }
}

fn now_json(ts: i64, units: &BTreeMap<String, String>, values: &BTreeMap<String, f64>) -> Value {
    let sensors: Vec<Value> = units
        .iter()
        .map(|(name, unit)| json!({"name": name, "unit": unit, "value": values.get(name)}))
        .collect();
    json!({"ts": ts, "sensors": sensors})
}

async fn get_now(State(app): State<App>) -> Result<Json<Value>, ApiError> {
    // Spawned so a client disconnecting mid-read can't leave a stale reply for the next read.
    let now = tokio::spawn(async move {
        let mut sensors = app.sensors.lock().await;
        let (ts, values) = sensors.snapshot(&app.db).await?;
        Some(now_json(ts, &sensors.units, &values))
    })
    .await;
    match now {
        Ok(Some(now)) => Ok(Json(now)),
        _ => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "sensors unavailable"})),
        )),
    }
}

fn err(code: StatusCode, msg: &str) -> ApiError {
    (code, Json(json!({ "error": msg })))
}

/// `GARDEN_ADMINS`: Tailscale logins separated by commas.
fn parse_admins(list: &str) -> Vec<String> {
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
        .filter(|l| app.admins.iter().any(|a| a == l))
        .map(String::from)
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "admins only"))
}

async fn get_me(State(app): State<App>, headers: HeaderMap) -> Json<Value> {
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
struct Bed {
    #[serde(skip_deserializing)]
    id: i64,
    name: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Planting {
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

async fn get_beds(State(app): State<App>) -> Result<Json<Vec<Bed>>, ApiError> {
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

async fn post_bed(
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

async fn patch_bed(
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

async fn delete_bed(
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
struct PlantingsParams {
    #[serde(default)]
    current: u8,
}

async fn get_plantings(
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

async fn post_planting(
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

async fn patch_planting(
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

async fn delete_planting(
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

async fn shutdown() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Ctrl-C handler") };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

#[tokio::main]
async fn main() {
    let mut conn = open("garden.db").expect("open garden.db");
    migrate(&mut conn).expect("run migrations");

    let state = App {
        db: Arc::new(Mutex::new(conn)),
        sensors: Default::default(),
        admins: Arc::new(parse_admins(
            &std::env::var("GARDEN_ADMINS").unwrap_or_default(),
        )),
    };
    eprintln!("admins: {}", state.admins.len());
    tokio::spawn(record_every_minute(state.clone()));

    let app = axum::Router::new()
        .route("/api/history", axum::routing::get(get_history))
        .route("/api/now", axum::routing::get(get_now))
        .route("/api/me", axum::routing::get(get_me))
        .route("/api/beds", axum::routing::get(get_beds).post(post_bed))
        .route(
            "/api/beds/{id}",
            axum::routing::patch(patch_bed).delete(delete_bed),
        )
        .route(
            "/api/plantings",
            axum::routing::get(get_plantings).post(post_planting),
        )
        .route(
            "/api/plantings/{id}",
            axum::routing::patch(patch_planting).delete(delete_planting),
        )
        .fallback_service(ServeDir::new("web/dist"))
        .with_state(state.clone());
    // Localhost only: tailscale serve is the sole access control (ADR 0002).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("bind 127.0.0.1:8080");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server error");

    eprintln!("shutting down");
    // Waits for a snapshot or save in progress; kept until exit so no new snapshot starts Python again.
    let mut sensors = state.sensors.lock().await;
    sensors.python = None; // kill_on_drop stops it
    eprintln!("stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> u32 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrations_apply_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 2);
        let tables: u32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('sensors', 'readings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);

        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 2);
    }

    #[test]
    fn readings_value_is_strict() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute("INSERT INTO sensors (id, name) VALUES (1, 'soil')", [])
            .unwrap();
        assert!(
            conn.execute("INSERT INTO readings VALUES (0, 1, 'wet')", [])
                .is_err()
        );
    }

    fn db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO sensors (id, name, unit) VALUES (1, 'soil', '%'), (2, 'temp', NULL), (3, 'old', 'x');",
        )
        .unwrap();
        conn
    }

    fn add(conn: &Connection, ts: i64, sensor: i64, value: f64) {
        conn.execute(
            "INSERT INTO readings VALUES (?1, ?2, ?3)",
            (ts, sensor, value),
        )
        .unwrap();
    }

    #[test]
    fn bucket_formula() {
        assert_eq!(bucket_size(0, 600), Some(60));
        assert_eq!(bucket_size(0, 86_400), Some(87));
        let (from, to) = (1_700_000_123, 1_700_000_123 + 366 * 86_400);
        let h = history(&db(), from, to, bucket_size(from, to).unwrap()).unwrap();
        assert!(h.t.len() <= 1001, "{}", h.t.len());
    }

    #[test]
    fn to_not_after_from_is_rejected() {
        assert_eq!(bucket_size(100, 100), None);
        assert_eq!(bucket_size(100, 50), None);
        assert_eq!(bucket_size(i64::MIN, i64::MAX), None);
    }

    #[test]
    fn history_aligns_averages_and_filters() {
        let conn = db();
        add(&conn, 59, 1, 1.0); // before `from`
        add(&conn, 120, 1, 10.0);
        add(&conn, 150, 1, 20.0); // same bucket as 120
        add(&conn, 240, 1, 5.0); // bucket 180 left empty
        add(&conn, 180, 2, 7.0);
        add(&conn, 301, 2, 9.0); // after `to`
        add(&conn, 1000, 3, 1.0); // `old` has no data in range
        let h = history(&conn, 60, 300, 60).unwrap();
        assert_eq!(h.t, vec![60, 120, 180, 240, 300]);
        let got: Vec<_> = h
            .sensors
            .iter()
            .map(|s| (s.name.as_str(), s.values.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("soil", vec![None, Some(15.0), None, Some(5.0), None]),
                ("temp", vec![None, None, Some(7.0), None, None]),
            ]
        );
    }

    #[test]
    fn history_json_shape() {
        let conn = db();
        add(&conn, 60, 1, 41.0);
        add(&conn, 120, 2, 20.5);
        let h = history(&conn, 60, 120, 60).unwrap();
        assert_eq!(
            serde_json::to_value(&h).unwrap(),
            json!({"bucket": 60, "t": [60, 120], "sensors": [
                {"name": "soil", "unit": "%", "values": [41.0, null]},
                {"name": "temp", "unit": null, "values": [null, 20.5]},
            ]})
        );
    }

    fn units() -> BTreeMap<String, String> {
        BTreeMap::from([("soil".into(), "%".into()), ("temp".into(), "°C".into())])
    }

    fn read(line: &str) -> BTreeMap<String, f64> {
        read_values(json_object(line).unwrap(), &units())
    }

    #[test]
    fn read_null_means_no_value() {
        assert_eq!(
            read(r#"{"soil":41.2,"temp":null}"#),
            BTreeMap::from([("soil".into(), 41.2)])
        );
    }

    #[test]
    fn read_drops_unknown_names() {
        assert_eq!(
            read(r#"{"soil":1.5,"wind":3}"#),
            BTreeMap::from([("soil".into(), 1.5)])
        );
    }

    #[test]
    fn read_drops_text_bools_and_objects() {
        assert!(read(r#"{"soil":"wet","temp":true}"#).is_empty());
        assert!(read(r#"{"soil":{"v":1}}"#).is_empty());
    }

    #[test]
    fn read_accepts_ints() {
        assert_eq!(
            read(r#"{"temp":18}"#),
            BTreeMap::from([("temp".into(), 18.0)])
        );
    }

    #[test]
    fn non_object_lines_are_skipped() {
        for line in ["debug: hello", "", "[1,2]", "42", "null", r#"{"soil":NaN}"#] {
            assert_eq!(json_object(line), None, "{line}");
        }
    }

    #[test]
    fn describe_accepts_names_to_units() {
        let reply = json_object(r#"{"soil":"%","temp":"°C"}"#).unwrap();
        assert_eq!(describe_units(reply), Some(units()));
    }

    #[test]
    fn describe_rejects_non_string_unit() {
        assert_eq!(
            describe_units(json_object(r#"{"soil":"%","temp":5}"#).unwrap()),
            None
        );
        assert_eq!(
            describe_units(json_object(r#"{"soil":null}"#).unwrap()),
            None
        );
    }

    #[test]
    fn upsert_sensor_updates_unit_keeps_id() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let soil = |conn: &Connection| -> (i64, String) {
            conn.query_row(
                "SELECT id, unit FROM sensors WHERE name = 'soil'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        upsert_sensors(&conn, &units()).unwrap();
        let (id, unit) = soil(&conn);
        assert_eq!(unit, "%");
        upsert_sensors(&conn, &BTreeMap::from([("soil".into(), "VWC %".into())])).unwrap();
        assert_eq!(soil(&conn), (id, "VWC %".into()));
    }

    fn saved(conn: &Connection) -> Vec<(i64, i64, f64)> {
        let mut stmt = conn
            .prepare("SELECT ts, sensor_id, value FROM readings ORDER BY ts, sensor_id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn save_snapshot_inserts_rows() {
        let mut conn = db();
        save_snapshot(
            &mut conn,
            60,
            &BTreeMap::from([("soil".into(), 41.2), ("temp".into(), 18.5)]),
        )
        .unwrap();
        assert_eq!(saved(&conn), vec![(60, 1, 41.2), (60, 2, 18.5)]);
    }

    #[test]
    fn save_snapshot_ignores_duplicate_ts() {
        let mut conn = db();
        save_snapshot(&mut conn, 60, &BTreeMap::from([("soil".into(), 1.0)])).unwrap();
        save_snapshot(&mut conn, 60, &BTreeMap::from([("soil".into(), 2.0)])).unwrap();
        assert_eq!(saved(&conn), vec![(60, 1, 1.0)]);
    }

    #[test]
    fn save_snapshot_ignores_unknown_name() {
        let mut conn = db();
        save_snapshot(&mut conn, 60, &BTreeMap::from([("wind".into(), 3.0)])).unwrap();
        assert!(saved(&conn).is_empty());
    }

    #[test]
    fn save_snapshot_is_all_or_nothing() {
        let mut conn = db();
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail BEFORE INSERT ON readings WHEN NEW.value = 999
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
        // soil is saved first, then temp fails: soil must be rolled back too.
        let values = BTreeMap::from([("soil".into(), 1.0), ("temp".into(), 999.0)]);
        assert!(save_snapshot(&mut conn, 60, &values).is_err());
        assert!(saved(&conn).is_empty());
    }

    #[test]
    fn next_minute_delay() {
        let ms = Duration::from_millis;
        assert_eq!(until_next_minute(ms(1_763_020_800_000)), ms(60_000)); // exactly :00
        assert_eq!(until_next_minute(ms(1_763_020_800_001)), ms(59_999));
        assert_eq!(until_next_minute(ms(1_763_020_859_500)), ms(500));
        assert_eq!(until_next_minute(ms(1_763_020_830_000)), ms(30_000));
    }

    #[test]
    fn now_json_shape() {
        let values = BTreeMap::from([("soil".into(), 41.2)]); // temp missing from the reply
        assert_eq!(
            now_json(1763020800, &units(), &values),
            json!({"ts": 1763020800, "sensors": [
                {"name": "soil", "unit": "%", "value": 41.2},
                {"name": "temp", "unit": "°C", "value": null},
            ]})
        );
    }

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
