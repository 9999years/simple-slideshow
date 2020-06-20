use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::string::FromUtf8Error;

use handlebars::{Handlebars, TemplateRenderError};
use pulldown_cmark::{html, Options, Parser};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RenderError {
    #[error("Error reading {0}: {1}")]
    Read(PathBuf, io::Error),

    #[error("Error rendering template: {0}")]
    Render(#[from] TemplateRenderError),

    #[error("Template produced invalid UTF-8: {0}")]
    Utf8(#[from] FromUtf8Error),
}

pub fn render(input_file: PathBuf, template: PathBuf) -> Result<String, RenderError> {
    let input = read(input_file)?;
    let mut template = read(template)?;

    let mut options = Options::empty();
    let parser = Parser::new_ext(&input, options);

    let mut markdown_html = String::with_capacity(input.len() * 2);
    html::push_html(&mut markdown_html, parser);

    let mut html_output = Vec::<u8>::with_capacity(template.len() + markdown_html.len());
    let mut reg = Handlebars::new();
    reg.render_template_source_to_write(&mut template.as_bytes(), &(), &mut html_output)?;

    Ok(String::from_utf8(html_output)?)
}

fn read(path: PathBuf) -> Result<String, RenderError> {
    let mut file = File::open(&path).map_err(|e| RenderError::Read(path.clone(), e))?;
    let len = file.metadata().map(|m| m.len() as usize).unwrap_or(4096);
    let mut content = String::with_capacity(len);
    file.read_to_string(&mut content)
        .map_err(|e| RenderError::Read(path, e))
        .map(|_| content)
}
