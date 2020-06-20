use std::error;
use std::path::{Path, PathBuf, StripPrefixError};
use std::{fs, io};

use structopt::StructOpt;
use thiserror::Error;

mod markdown;

#[derive(Debug, StructOpt)]
#[structopt(about = "A Markdown-based slideshow rendering tool.")]
struct Opt {
    /// Watch for changes to files and keep re-rendering?
    #[structopt(short, long)]
    watch: bool,

    /// Directory of static files, copied unmodified into the output
    /// directory.
    #[structopt(long, parse(from_os_str), default_value = "static")]
    static_dir: PathBuf,

    /// Slideshow template.
    #[structopt(long, parse(from_os_str), default_value = "template.html")]
    template: PathBuf,

    /// Input Markdown file.
    #[structopt(parse(from_os_str))]
    input: PathBuf,

    /// Output directory.
    #[structopt(parse(from_os_str), default_value = "out")]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn error::Error>> {
    let opt = Opt::from_args();

    if opt.watch {
        watch(opt)?;
    } else {
        build(opt)?;
    }
    Ok(())
}

#[derive(Error, Debug)]
enum CopyStaticErr {
    #[error("Error travering static files directory: {0}")]
    WalkDir(#[from] walkdir::Error),

    #[error("Error travering static files directory: {0}")]
    Prefix(#[from] StripPrefixError),

    #[error("Error traversing static files directory, while copying {from} to {to}: {err}")]
    Copy {
        from: PathBuf,
        to: PathBuf,
        err: io::Error,
    },

    #[error("Error traversing static files directory, while creating {dir}: {err}")]
    CreateDir { dir: PathBuf, err: io::Error },
}

fn copy_static(static_dir: &Path, output_dir: &Path) -> Result<(), CopyStaticErr> {
    use walkdir::WalkDir;

    for entry in WalkDir::new(static_dir).follow_links(true) {
        let path = entry?.into_path();
        let rel = path.strip_prefix(static_dir)?;
        let dest = output_dir.join(rel);
        if path.is_dir() {
            fs::create_dir_all(&dest)
                .map_err(|e| CopyStaticErr::CreateDir { dir: dest, err: e })?;
        } else {
            fs::copy(&path, &dest).map_err(|e| CopyStaticErr::Copy {
                from: path,
                to: dest,
                err: e,
            })?;
        }
    }
    Ok(())
}

#[derive(Error, Debug)]
enum BuildErr {
    #[error("{0}")]
    CopyStatic(#[from] CopyStaticErr),

    #[error("{0}")]
    Render(#[from] markdown::RenderError),
}

fn build(opt: Opt) -> Result<(), BuildErr> {
    copy_static(&opt.static_dir, &opt.output)?;

    let res = markdown::render(opt.input, opt.template)?;

    println!("{}", res);

    Ok(())
}

#[derive(Error, Debug)]
enum WatchErr {
    #[error("{0}")]
    CopyStatic(#[from] CopyStaticErr),

    #[error("{0}")]
    Recv(#[from] std::sync::mpsc::RecvError),

    #[error("{0}")]
    Notify(notify::Error, Option<PathBuf>),
}

fn watch(opt: Opt) -> Result<(), WatchErr> {
    copy_static(&opt.static_dir, &opt.output)?;

    use notify::{watcher, DebouncedEvent, RecursiveMode, Watcher};
    use std::time::Duration;

    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = watcher(tx, Duration::from_millis(250)).unwrap();

    watcher
        .watch(opt.static_dir, RecursiveMode::Recursive)
        .unwrap();
    watcher
        .watch(opt.input, RecursiveMode::NonRecursive)
        .unwrap();
    watcher
        .watch(opt.template, RecursiveMode::NonRecursive)
        .unwrap();

    loop {
        match rx.recv()? {
            DebouncedEvent::Create(path) => {}
            DebouncedEvent::Write(path) => {}
            DebouncedEvent::Chmod(path) => {}
            DebouncedEvent::Remove(path) => {}
            DebouncedEvent::Rename(from, to) => {}
            DebouncedEvent::Rescan => {}
            DebouncedEvent::Error(err, path) => {
                return Err(WatchErr::Notify(err, path));
            }
            _ => {}
        }
    }
}
