//! Running sensors.py, the every-minute recorder, and /api/now.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::{Connection, named_params};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::{ApiError, App, Error};

// The venv's python directly, not `uv run`: killing a `uv` wrapper can leave a hung python holding the sensors.
const DEFAULT_SENSORS_CMD: &str = "sensors/.venv/bin/python sensors/sensors.py";

/// The running `sensors.py` child.
pub struct Python {
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
pub async fn record_every_minute(app: App) {
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
pub struct Sensors {
    pub python: Option<Python>,
    pub units: BTreeMap<String, String>, // from the latest describe
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

pub async fn get_now(State(app): State<App>) -> Result<Json<Value>, ApiError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;
    use crate::db::tests::db;

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
}
