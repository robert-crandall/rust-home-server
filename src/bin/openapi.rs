use std::fs;
use std::path::PathBuf;

use app::routes::openapi_spec;

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/openapi.json"));

    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).expect("create docs directory");
    }

    let mut json = openapi_spec()
        .to_pretty_json()
        .expect("serialize OpenAPI spec");
    json.push('\n');

    fs::write(&out_path, json).expect("write OpenAPI spec");
    println!("wrote {}", out_path.display());
}
