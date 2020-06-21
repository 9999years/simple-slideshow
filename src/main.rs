use std::error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf, StripPrefixError};

use structopt::StructOpt;
use thiserror::Error;
use tracing::{event, info, instrument, span, warn, Level};
use tracing_subscriber;

mod markdown;

#[derive(Debug, StructOpt)]
#[structopt(about = "A Markdown-based slideshow rendering tool.")]
struct Opt {
    /// Log level.
    ///
    /// Can be an integer 1-5 or "error", "warn", "info", "debug", "trace",
    /// case-insensitive.
    #[structopt(long, default_value = "warn")]
    trace_level: Level,

    /// Watch for changes to files and keep re-rendering?
    #[structopt(short, long)]
    watch: bool,

    /// Debounce filesystem events to a given granularity, in milliseconds.
    #[structopt(long, default_value = "250")]
    debounce_ms: u64,

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
    output_dir: PathBuf,
}

fn main() {
    if let Err(e) = main_inner() {
        println!("{}", e);
    }
}

fn main_inner() -> Result<(), Box<dyn error::Error>> {
    use tracing_subscriber::fmt::format::Format;

    let opt = {
        let mut opt = Opt::from_args();
        opt.static_dir = opt
            .static_dir
            .canonicalize()
            .expect("Canonicalize static_dir");
        opt.template = opt.template.canonicalize().expect("Canonicalize template");
        opt.input = opt.input.canonicalize().expect("Canonicalize input");
        opt.output_dir = opt
            .output_dir
            .canonicalize()
            .expect("Canonicalize output_dir");
        opt
    };

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(opt.trace_level.clone())
        // .event_format(Format::default().compact())
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("setting tracing default subscriber failed");

    if opt.watch {
        opt.watch()?;
    } else {
        opt.render()?;
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

#[derive(Error, Debug)]
enum BuildErr {
    #[error("{0}")]
    CopyStatic(#[from] CopyStaticErr),

    #[error("{0}")]
    Render(#[from] markdown::RenderError),

    #[error("Error creating output file {0}: {1}")]
    OutputFile(PathBuf, io::Error),

    #[error("Error writing output file {0}: {1}")]
    OutputWrite(PathBuf, io::Error),
}

#[derive(Error, Debug)]
enum WatchErr {
    #[error("{0}")]
    CopyStatic(#[from] CopyStaticErr),

    #[error("{0}")]
    Recv(#[from] std::sync::mpsc::RecvError),

    #[error("{0}")]
    Notify(notify::Error, Option<PathBuf>),

    #[error("{0}")]
    Build(#[from] BuildErr),
}

impl Opt {
    fn output_file(&self) -> PathBuf {
        self.output_dir.join("index.html")
    }

    #[instrument(skip(self), err)]
    fn copy_single_static(&self, path: PathBuf) -> Result<(), CopyStaticErr> {
        let rel = path.strip_prefix(&self.static_dir)?;
        let dest = self.output_dir.join(rel);
        if path.is_dir() {
            if !dest.exists() {
                event!(Level::INFO, created_dir = ?dest);
                fs::create_dir_all(&dest)
                    .map_err(|e| CopyStaticErr::CreateDir { dir: dest, err: e })?;
            }
        } else {
            event!(Level::INFO, from = ?path, to = ?dest);
            fs::copy(&path, &dest).map_err(|e| CopyStaticErr::Copy {
                from: path,
                to: dest,
                err: e,
            })?;
        }
        Ok(())
    }

    #[instrument(skip(self))]
    fn copy_static(&self) -> Result<(), CopyStaticErr> {
        use walkdir::WalkDir;

        for entry in WalkDir::new(&self.static_dir).follow_links(true) {
            let path = entry?.into_path();
            event!(Level::INFO, ?path);
            self.copy_single_static(path)?;
        }
        Ok(())
    }

    fn make_output_dir(&self) -> Result<(), BuildErr> {
        make_output(&self.output_dir).map_err(|e| BuildErr::OutputFile(self.output_dir.clone(), e))
    }

    fn render(&self) -> Result<(), BuildErr> {
        self.copy_static()?;
        self.make_output_dir()?;
        self.write_markdown_file()?;
        Ok(())
    }

    fn render_markdown_string(&self) -> Result<String, BuildErr> {
        Ok(markdown::render(&self.input, &self.template)?)
    }

    fn write_markdown_file(&self) -> Result<(), BuildErr> {
        let res = self.render_markdown_string()?;
        let output = self.output_file();
        let mut file =
            File::create(&output).map_err(|e| BuildErr::OutputFile(output.clone(), e))?;
        write!(&mut file, "{}", res).map_err(|e| BuildErr::OutputWrite(output, e))?;
        Ok(())
    }

    #[instrument(skip(self))]
    fn watch(&self) -> Result<(), WatchErr> {
        use notify::{watcher, DebouncedEvent, RecursiveMode, Watcher};
        use std::time::Duration;

        self.render()?;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = watcher(tx, Duration::from_millis(self.debounce_ms)).unwrap();

        watcher
            .watch(&self.static_dir, RecursiveMode::Recursive)
            .unwrap();
        watcher
            .watch(
                &self.input.parent().unwrap_or(&self.input),
                RecursiveMode::NonRecursive,
            )
            .unwrap();
        watcher
            .watch(
                &self.template.parent().unwrap_or(&self.template),
                RecursiveMode::NonRecursive,
            )
            .unwrap();

        event!(Level::INFO, "initialized filesystem watcher");

        loop {
            let event = {
                let span = span!(Level::INFO, "watch");
                let _guard = span.enter();
                rx.recv()?
            };
            let span = span!(Level::INFO, "filesystem event", event = ?event);
            let _guard = span.enter();
            event!(Level::INFO, ?event);
            match event {
                DebouncedEvent::Create(path) | DebouncedEvent::Write(path) => {
                    if path.starts_with(&self.static_dir) {
                        self.copy_single_static(path)?;
                    } else if &path == &self.input || &path == &self.template {
                        self.write_markdown_file()?;
                    }
                }
                DebouncedEvent::Chmod(path) => {
                    if path.starts_with(&self.static_dir) {
                        self.copy_single_static(path)?;
                    } else {
                        self.write_markdown_file()?;
                    }
                }
                DebouncedEvent::Remove(path) => {
                    event!(Level::WARN, "remove (unimplemented)");
                }
                DebouncedEvent::Rename(from, to) => {
                    event!(Level::WARN, "rename (unimplemented)");
                }
                DebouncedEvent::Rescan => {
                    event!(Level::INFO, "rescanning watched files");
                }
                DebouncedEvent::Error(err, path) => {
                    if let Some(path) = &path {
                        event!(Level::ERROR, ?path);
                    }
                    return Err(WatchErr::Notify(err, path));
                }
                _ => {
                    event!(Level::DEBUG, "unhandled event");
                }
            }
        }
    }
}

#[instrument(err)]
fn make_output(output_dir: &Path) -> io::Result<()> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)
    } else {
        Ok(())
    }
}
