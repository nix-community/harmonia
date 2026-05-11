use std::fs::{Permissions, create_dir_all, set_permissions, write};
use std::io::{self, Cursor};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use bytes::Bytes;

use super::CASE_HACK_SUFFIX;

use super::NarEvent;

pub type TestNarEvent = NarEvent<Cursor<Bytes>>;
pub type TestNarEvents = Vec<TestNarEvent>;

pub fn text_file() -> TestNarEvents {
    vec![NarEvent::File {
        name: Bytes::new(),
        executable: false,
        size: 12,
        reader: Cursor::new(Bytes::from_static(b"Hello world!")),
    }]
}

pub fn exec_file() -> TestNarEvents {
    vec![NarEvent::File {
        name: Bytes::new(),
        executable: true,
        size: 15,
        reader: Cursor::new(Bytes::from_static(b"Very cool stuff")),
    }]
}

pub fn empty_file() -> TestNarEvents {
    vec![NarEvent::File {
        name: Bytes::new(),
        executable: false,
        size: 0,
        reader: Cursor::new(Bytes::new()),
    }]
}

pub fn empty_file_in_dir() -> TestNarEvents {
    vec![
        NarEvent::StartDirectory { name: Bytes::new() },
        NarEvent::File {
            name: Bytes::from_static(b"a=?.0.aA"),
            executable: false,
            size: 0,
            reader: Cursor::new(Bytes::new()),
        },
        NarEvent::EndDirectory,
    ]
}

pub fn empty_dir() -> TestNarEvents {
    vec![
        NarEvent::StartDirectory { name: Bytes::new() },
        NarEvent::EndDirectory,
    ]
}

pub fn empty_dir_in_dir() -> TestNarEvents {
    vec![
        NarEvent::StartDirectory { name: Bytes::new() },
        NarEvent::StartDirectory {
            name: Bytes::from_static(b"empty"),
        },
        NarEvent::EndDirectory,
        NarEvent::EndDirectory,
    ]
}

pub fn symlink() -> TestNarEvents {
    vec![NarEvent::Symlink {
        name: Bytes::new(),
        target: Bytes::from_static(b"../deep"),
    }]
}

pub fn dir_example() -> TestNarEvents {
    /*
    Offsets:

    NAR_VERSION_MAGIC_1 // 24

    "(", // 16
    "type", // 16
    "directory", // 24
    // Size -> offset: 56 -> 80

    "entry", // 16
    "(" // 16
    "name" // 16
    "dir" // 16
    "node" // 16
    // Size -> offset: 80 -> 160

    "(" // 16
    "type" // 16
    "directory" // 24
    // Size -> offset: 56 -> 216

    "entry", // 16
    "(" // 16
    "name" // 16
    "more" // 16
    "node" // 16
    // Size -> offset: 80 -> 296

    "(" // 16
    "type" // 16
    "directory" // 24
    // Size -> offset: 56 -> 352

    "entry" // 16
    "("// 16
    "name" // 16
    "Deep" // 16
    "node" // 16
    // Size -> offset: 80 -> 432

    "(" // 16
    "type" // 16
    "regular" // 16
    "executable", // 24
    "" // 8
    "contents", // 16
    8
    // Size -> offset: 104 -> 536

    */
    vec![
        NarEvent::StartDirectory { name: Bytes::new() },
        NarEvent::StartDirectory {
            name: Bytes::from_static(b"dir"),
        },
        NarEvent::StartDirectory {
            name: Bytes::from_static(b"more"),
        },
        NarEvent::File {
            name: Bytes::from_static(b"Deep"),
            executable: true,
            size: 15,
            reader: Cursor::new(Bytes::from_static(b"Very cool stuff")),
        },
        NarEvent::StartDirectory {
            name: Bytes::from_static(b"deep"),
        },
        NarEvent::File {
            name: Bytes::from_static(b"empty.keep"),
            executable: false,
            size: 0,
            reader: Cursor::new(Bytes::new()),
        },
        NarEvent::Symlink {
            name: Bytes::from_static(b"loop"),
            target: Bytes::from_static(b"../deep"),
        },
        NarEvent::Symlink {
            name: Bytes::from_static(b"test"),
            target: Bytes::from_static(b"/etc/ssh/sshd_config"),
        },
        NarEvent::EndDirectory,
        NarEvent::EndDirectory,
        NarEvent::EndDirectory,
        NarEvent::File {
            name: Bytes::from_static(b"testing.txt"),
            executable: false,
            size: 12,
            reader: Cursor::new(Bytes::from_static(b"Hello world!")),
        },
        NarEvent::EndDirectory,
    ]
}

pub fn case_hack_sorting() -> TestNarEvents {
    use NarEvent::*;
    vec![
        StartDirectory {
            name: Bytes::from_static(b""),
        },
        File {
            name: Bytes::from_static(b"N"),
            executable: false,
            size: 0,
            reader: Cursor::new(Bytes::from_static(b"")),
        },
        File {
            name: Bytes::from_static(b"n"),
            executable: false,
            size: 0,
            reader: Cursor::new(Bytes::from_static(b"")),
        },
        File {
            name: Bytes::from_static(b"n?"),
            executable: false,
            size: 0,
            reader: Cursor::new(Bytes::from_static(b"")),
        },
        EndDirectory,
    ]
}

pub fn create_dir_example<P>(path: P, case_hack: bool) -> io::Result<()>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    create_dir_all(path)?;
    let more = path.join("dir").join("more");
    create_dir_all(&more)?;
    let deep_file = more.join("Deep");
    write(&deep_file, b"Very cool stuff")?;
    let permissions = Permissions::from_mode(0o700);
    set_permissions(deep_file, permissions)?;

    let deep = if case_hack {
        more.join(format!("deep{}{}", CASE_HACK_SUFFIX, 1))
    } else {
        more.join("deep")
    };
    create_dir_all(&deep)?;
    write(deep.join("empty.keep"), b"")?;
    std::os::unix::fs::symlink("../deep", deep.join("loop"))?;
    std::os::unix::fs::symlink("/etc/ssh/sshd_config", deep.join("test"))?;
    write(path.join("testing.txt"), b"Hello world!")?;
    Ok(())
}
