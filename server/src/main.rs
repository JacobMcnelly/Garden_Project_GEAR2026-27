use rusqlite::Connection;
use tower_http::services::ServeDir;

const MIGRATIONS: &[(u32, &str)] = &[(1, include_str!("../../db/migrations/0001_init.sql"))];

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

    let app = axum::Router::new().fallback_service(ServeDir::new("web/dist"));
    // Localhost only: tailscale serve is the sole access control (ADR 0002).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("bind 127.0.0.1:8080");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server error");
    drop(conn);
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
        assert_eq!(version(&conn), 1);
        let tables: u32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('sensors', 'readings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);

        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 1);
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
}
