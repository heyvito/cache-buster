use clap::{App, Arg};
use walkdir::WalkDir;
use std::collections::HashMap;
use std::path::{PathBuf, Path};
use std::{fs, io};
use sha1::{Sha1, Digest};
use std::io::{Error, Write};
use lol_html::{HtmlRewriter, Settings, element};
use url::Url;
use std::ffi::OsStr;


fn main() {
    let matches = App::new("cache-buster")
        .version("1.0")
        .author("Victor \"Vito\" Gama <hey@vito.io>")
        .about("Rewrites assets and HTML/CSS files to prevent wrong caching")
        .arg(Arg::with_name("assets")
            .short("a")
            .long("assets")
            .value_name("FILE")
            .help("Determines where assets are located")
            .takes_value(true)
            .required(true))
        .arg(Arg::with_name("base_url")
            .short("u")
            .long("base-url")
            .value_name("URL")
            .help("Your website's base url")
            .takes_value(true)
            .required(true))
        .arg(Arg::with_name("ignore_assets_extensions")
            .short("i")
            .long("ignore-assets-extensions")
            .value_name("LIST")
            .help("List of extensions to not hash")
            .takes_value(true)
            .multiple(true)
            .value_delimiter(","))
        .get_matches();

    let assets = normalize_path(matches.value_of("assets").unwrap());
    let pwd = std::env::current_dir().unwrap();
    let base_url = match Url::parse(matches.value_of("base_url").unwrap()) {
        Err(e) => {
            eprintln!("Error parsing base-url: {}", e);
            std::process::exit(1);
        }
        Ok(url) => url
    };
    let exts = matches.values_of_lossy("ignore_assets_extensions").unwrap_or_else(|| vec![]);

    match execute(pwd, assets, &base_url, exts) {
        Ok(_) => {}
        Err(e) => panic!(format!("Error: {}", e))
    }
}

fn hash_file(p: &Path) -> Result<String, Error> {
    let mut file = fs::File::open(p)?;
    let mut hasher = Sha1::new();
    io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

fn update_asset(path_str: &str, assets: &HashMap<String, String>) -> String {
    let hash = match assets.get(path_str) {
        Some(h) => h,
        None => return String::from(path_str)
    };

    let mut path = PathBuf::from(path_str);
    let stem = path.file_stem().unwrap_or_else(|| OsStr::new(""));
    let mut filename = stem.to_os_string();
    filename.push(format!("_{}", hash));
    if let Some(ext) = path.extension() {
        filename.push(format!(".{}", ext.to_str().unwrap()));
    }
    path.set_file_name(filename);

    String::from(path.to_str().unwrap())
}

fn normalize_path(path: &str) -> &str {
    match path.strip_prefix('/') {
        Some(stripped) => stripped,
        None => path
    }
}

fn match_asset(src: &str, base_url: &Url, assets_path: &str, assets: &HashMap<String, String>) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        let mut link = match Url::parse(src) {
            Ok(u) => u,
            Err(_) => return String::from(src),
        };
        let path = normalize_path(link.path());

        if link.has_host() && link.host_str().eq(&base_url.host_str()) && path.starts_with(assets_path) {
            let fixed_path = update_asset(path, assets);
            link.set_path(&fixed_path);
            return link.into_string();
        }
    }

    let src = normalize_path(src);
    if src.starts_with(assets_path) {
        return update_asset(src, assets);
    }

    String::from(src)
}

fn execute(source: PathBuf, assets_path: &str, base_url: &Url, ignored_exts: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut assets_hashes = HashMap::new();
    for entry in WalkDir::new(assets_path)
        .into_iter()
        .map(|el| el.unwrap())
        .filter(|el| el.metadata().unwrap().is_file())
        .filter(|el| !ignored_exts.contains(&String::from(el.path().extension().unwrap().to_str().unwrap()))) {
        let path = entry.path();
        let hash = match hash_file(path) {
            Ok(hash) => hash,
            Err(e) => {
                println!("Error hashing {}: {}", path.to_str().unwrap(), e);
                std::process::exit(1);
            }
        };
        let path_str = path.to_str().unwrap();
        assets_hashes.insert(path_str.to_string(), hash);
    };

    for entry in WalkDir::new(source) {
        let read_entry = entry?;
        let filename = read_entry.file_name().to_str().unwrap();
        let path = PathBuf::from(read_entry.path());
        if !filename.ends_with(".htm") && !filename.ends_with(".html") {
            continue;
        }

        let mut output = tempfile::NamedTempFile::new()?;
        let mut rewriter = HtmlRewriter::try_new(
            Settings {
                element_content_handlers: vec![
                    element!("script[src]", |el| {
                        let src = el.get_attribute("src").expect("expected src to be present");
                        let new_src = match_asset(&src, base_url, assets_path, &assets_hashes);
                        el.set_attribute("src", &new_src)?;
                        Ok(())
                    }),
                    element!("link[rel='stylesheet'][href]", |el| {
                        let href = el.get_attribute("href").expect("expected href to be present");
                        let new_href = match_asset(&href, base_url, assets_path, &assets_hashes);
                        el.set_attribute("href", &new_href)?;
                        Ok(())
                    }),
                    element!("img[src]", |el| {
                        let src = el.get_attribute("src").expect("expected src to be present");
                        let new_src = match_asset(&src, base_url, assets_path, &assets_hashes);
                        el.set_attribute("src", &new_src)?;
                        Ok(())
                    })
                ],
                ..Settings::default()
            }, |c: &[u8]| output.write_all(c).unwrap(),
        )?;
        rewriter.write(&* match fs::read(&path) {
            Ok(d) => d,
            Err(err) => {
                println!("Error reading {}: {}", path.to_str().unwrap(), err);
                std::process::exit(1);
            }
        })?;
        rewriter.end()?;

        if let Err(err) = fs::rename(&output, &path) {
            println!("Error renaming {} => {}: {}", output.path().to_str().unwrap(), path.to_str().unwrap(), err);
            std::process::exit(1);
        }
    }

    for file in assets_hashes.keys() {
        let new_name = update_asset(file, &assets_hashes);
        if let Err(err) = fs::rename(file, &new_name) {
            println!("Error renaming {} => {}: {}", file, new_name, err);
            std::process::exit(1);
        }
    }

    Ok(())
}
