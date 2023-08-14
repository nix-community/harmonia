use anyhow::{Context, Result};
use nix::errno::Errno;
use nix::fcntl::{flock, FlockArg};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::exit;

struct Lock {
    file: std::fs::File,
}
impl Drop for Lock {
    fn drop(&mut self) {
        let unlock = flock(self.file.as_raw_fd(), FlockArg::Unlock);
        match unlock {
            Ok(_) => {}
            Err(err) => {
                eprintln!("Failed to unlock file: {:?}", err);
            }
        }
    }
}

/// Locks file at path for writing using fcntl
/// once Self is dropped, the lock is released
fn lock_file<P: AsRef<Path>>(path: P) -> Result<Lock> {
    let file = std::fs::File::open(path.as_ref())
        .with_context(|| format!("Failed to open file {}", path.as_ref().display()))?;

    let res = flock(file.as_raw_fd(), FlockArg::LockSharedNonblock);
    match res {
        Ok(_) => Ok(Lock { file }),
        Err(Errno::EWOULDBLOCK) => {
            println!("waiting for the big garbage collector lock...");
            flock(file.as_raw_fd(), FlockArg::LockShared)
                .with_context(|| format!("Failed to lock file {}", path.as_ref().display()))?;
            Ok(Lock { file })
        }
        Err(err) => {
            Err(err).with_context(|| format!("Failed to lock file {}", path.as_ref().display()))
        }
    }
}

fn db_dir() -> PathBuf {
    let state_path = std::env::var("NIX_STATE_DIR");
    let path = state_path
        .as_ref()
        .map(|s| Path::new(s))
        .unwrap_or_else(|_| Path::new("/nix/var/nix/"));
    path.join("db")
}

pub struct Connection {
    conn: rusqlite::Connection,
    lock: Lock,
}

pub async fn connect() -> Result<Connection> {
    let lock_path = db_dir().join("big-lock");
    let lock = lock_file(&lock_path)?;

    let db_path = db_dir().join("db.sqlite");
    let conn = rusqlite::Connection::open(&db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    Ok(Connection { conn, lock })
}

// id               integer primary key autoincrement not null,
// path             text unique not null,
// hash             text not null,
// registrationTime integer not null,
// deriver          text,
// narSize          integer,
// ultimate         integer, -- null implies "false"
// sigs             text, -- space-separated
// ca               text -- if not null, an assertion that the path is content-addressed; see ValidPathInfo
//let mut res = vec![
//    format!("StorePath: {}", narinfo.store_path),
//    format!("URL: {}", narinfo.url),
//    format!("Compression: {}", narinfo.compression),
//    format!("FileHash: {}", narinfo.nar_hash),
//    format!("FileSize: {}", narinfo.nar_size),
//    format!("NarHash: {}", narinfo.nar_hash),
//    format!("NarSize: {}", narinfo.nar_size),
//];

//if !narinfo.references.is_empty() {
//    res.push(format!("References: {}", &narinfo.references.join(" ")));
//}

//if let Some(drv) = &narinfo.deriver {
//    res.push(format!("Deriver: {}", drv));
//}

//if let Some(sys) = &narinfo.system {
//    res.push(format!("System: {}", sys));
//}

//if let Some(sig) = &narinfo.sig {
//    res.push(format!("Sig: {}", sig));
//}

//if let Some(ca) = &narinfo.ca {
//    res.push(format!("CA: {}", ca));
//}

#[derive(Debug)]
pub struct PathInfo {
    path: String,
    hash: String,
    nar_size: u64,
    sigs: Option<String>,
    ca: Option<String>,
    deriver: Option<String>,
    references: Vec<String>,
}

impl Connection {
    pub fn path_info(&self, hash: &str) -> Result<Option<PathInfo>> {
        // FIXME fix query to also include results if there no references.
        let mut stmt = self.conn.prepare_cached(
            "select vp.path, vp.hash, vp.narSize, vp.sigs, vp.ca, vp.deriver, vp2.path from ValidPaths vp
               inner join Refs r1 on r1.referrer = vp.id
               inner join ValidPaths vp2 on r1.reference = vp2.id
               where vp.path >= :lower and vp.path <= :upper",
        ).context("Failed to prepare statement")?;

        let lower = format!("/nix/store/{}-", hash);
        let upper = format!("/nix/store/{}.", hash);

        // hack to only get one store path at the time
        let mut rows = stmt.query(&[(":lower", &lower), (":upper", &upper)])?;
        let mut path_info = if let Some(row) = rows.next()? {
            PathInfo {
                path: row.get(0)?,
                hash: row.get(1)?,
                nar_size: row.get(2)?,
                sigs: row.get(3)?,
                ca: row.get(4)?,
                deriver: row.get(5)?,
                references: vec![row.get(6)?],
            }
        } else {
            return Ok(None);
        };
        while let Some(row) = rows.next()? {
            path_info.references.push(row.get(6)?);
        }

        Ok(Some(path_info))
    }
}

#[cfg(test)]
mod tests {
    use super::connect;

    #[tokio::test]
    async fn test_connect() {
        let conn = connect().await.unwrap();
        conn.path_info("qiqczfp5bq19bfnbizz0zxl9vjbrarck").unwrap();
    }
}
