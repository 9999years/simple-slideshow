use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::string::FromUtf8Error;

use handlebars::{Handlebars, TemplateRenderError};
use pulldown_cmark::{html, Event, Options, Parser};
use serde::Serialize;
use thiserror::Error;
use tracing::{event, instrument, span, Level};

#[derive(Error, Debug)]
pub enum RenderError {
    #[error("Error reading {0}: {1}")]
    Read(PathBuf, io::Error),

    #[error("Error rendering template: {0}")]
    Render(#[from] TemplateRenderError),

    #[error("Template produced invalid UTF-8: {0}")]
    Utf8(#[from] FromUtf8Error),
}

#[instrument(err)]
pub fn render(
    input_file: impl AsRef<Path> + fmt::Debug,
    template: impl AsRef<Path> + fmt::Debug,
) -> Result<String, RenderError> {
    let input = read(input_file)?;
    let template = read(template)?;

    let (rendered_markdown, mut html_output) = {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_TABLES);
        let parser = Slideshow::new(Parser::new_ext(&input, options));

        let span = span!(Level::INFO, "render_markdown");
        let _guard = span.enter();
        let mut markdown_html = String::with_capacity(input.len() * 2);
        html::push_html(&mut markdown_html, parser);
        let html_output = Vec::<u8>::with_capacity(template.len() + markdown_html.len());
        (markdown_html, html_output)
    };

    let ctx = TemplateContext {
        content: rendered_markdown,
    };

    let span = span!(Level::INFO, "render_handlebars");
    let _guard = span.enter();
    let reg = Handlebars::new();
    reg.render_template_source_to_write(&mut template.as_bytes(), &ctx, &mut html_output)?;

    Ok(String::from_utf8(html_output)?)
}

#[derive(Serialize, Debug)]
struct TemplateContext {
    content: String,
}

fn read(path: impl AsRef<Path>) -> Result<String, RenderError> {
    let mut file = File::open(&path).map_err(|e| RenderError::Read(path.as_ref().into(), e))?;
    let len = file.metadata().map(|m| m.len() as usize).unwrap_or(4096);
    let mut content = String::with_capacity(len);
    file.read_to_string(&mut content)
        .map_err(|e| RenderError::Read(path.as_ref().into(), e))
        .map(|_| content)
}

struct Slideshow<'a> {
    parser: Parser<'a>,
    next_events: VecDeque<Event<'a>>,
    in_slide: bool,
}

impl<'a> Slideshow<'a> {
    fn new(parser: Parser<'a>) -> Self {
        let mut ret = Self {
            parser,
            next_events: Default::default(),
            in_slide: false,
        };
        ret.start_slide();
        ret
    }

    fn start_slide(&mut self) {
        self.in_slide = true;
        self.next_events
            .push_back(Event::Html(r#"<section class="slide">"#.into()));
    }

    fn end_slide(&mut self) {
        self.next_events
            .push_back(Event::Html(r#"</section>"#.into()));
        self.in_slide = false;
    }

    fn transform(&mut self, event: Event<'a>) {
        match event {
            Event::Rule => {
                if self.in_slide {
                    self.end_slide();
                }
                self.start_slide();
            }
            _ => {
                self.next_events.push_back(event);
            }
        }
    }
}

impl<'a> Iterator for Slideshow<'a> {
    type Item = <Parser<'a> as Iterator>::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_events.pop_front().or_else(|| {
            let event = self.parser.next()?;
            self.transform(event);
            self.next()
        })
    }
}
