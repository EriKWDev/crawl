use std::{
    collections::{HashMap, HashSet},
    io::Read,
    sync::{Arc, Mutex},
};

use base64::Engine;
use http_req::request;
use url::Url;

fn get_url(url: &Url, buffer: &mut Vec<u8>) -> bool {
    let db = std::path::PathBuf::from("db");
    let url_str = url.as_str();
    let str = base64::prelude::BASE64_URL_SAFE.encode(url_str);
    let str = format!("{}{str}", url.host_str().unwrap_or(""));
    let folder = db.join(str);
    let _ = std::fs::create_dir_all(&folder);
    let file_path = folder.join("index.html.lz4");

    if let Ok(mut f) = std::fs::File::open(&file_path) {
        println!("READ {file_path:?}");
        let res = f.read_to_end(buffer).is_ok();

        if !res {
            return false;
        }

        let decompressed = lz4_flex::decompress_size_prepended(&buffer).unwrap();
        buffer.clear();
        buffer.extend_from_slice(&decompressed);
        return true;
    }

    println!("GET {url_str}");

    let Ok(res) = request::get(&url, buffer) else {
        return false;
    };

    let compressed = lz4_flex::compress_prepend_size(&buffer);
    let _ = std::fs::write(&file_path, &compressed);

    true
}

fn crawler_work(
    message: &Url,
    checked_urls: &Arc<Mutex<HashSet<Url>>>,
    word_count: &Arc<Mutex<HashMap<String, usize>>>,
    buffer: &mut Vec<u8>,
    to_crawl: &crossbeam_channel::Sender<Url>,
) {
    let mut url = message.clone();
    url.set_query(None);
    url.set_fragment(None);

    if is_forbidden_url(&url) {
        return;
    }

    let mut checked = checked_urls.lock().unwrap();
    if !checked.insert(url.clone()) {
        return;
    }
    drop(checked);

    if !get_url(&url, buffer) {
        return;
    }

    let body = unsafe { std::str::from_utf8_unchecked(&buffer) };

    let res = tl::parse(body, tl::ParserOptions::default());
    if let Ok(res) = res {
        let p = res.parser();

        if let Some(them) = res.query_selector("a") {
            for it in them.into_iter() {
                if let Some(tl::Node::Tag(tag)) = it.get(p) {
                    let attr = tag.attributes();
                    let it = attr.get("href");
                    if let Some(Some(it)) = it {
                        let it = it.as_utf8_str();

                        match url::Url::parse(&it) {
                            Ok(new) => {
                                let _ = to_crawl.send(new);
                            }

                            Err(_) => {
                                if let Ok(url) = url.join(&it) {
                                    let _ = to_crawl.send(url);
                                }
                            }
                        }
                    }
                }
            }
        }

        if DO_WORD_COUNT {
            if let Some(them) = res.query_selector("a, p, h1, h2, h3, h4, h5, h6, span") {
                let mut local = HashMap::<String, usize>::new();
                for it in them.into_iter() {
                    if let Some(it) = it.get(p) {
                        if let Some(r) = it.as_raw() {
                            let text = r.as_utf8_str();
                            for word in text.split_whitespace() {
                                let word = word.to_lowercase();
                                let it = local.entry(word).or_insert(0);
                                *it += 1;
                            }
                        }
                    }
                }
                let mut global = word_count.lock().unwrap();
                for (key, value) in local {
                    let it = global.entry(key.to_string()).or_default();
                    *it = *it + value;
                }
                drop(global);
            }
        }
    }
}

pub fn report(word_count: &Arc<Mutex<HashMap<String, usize>>>, n: Option<usize>) {
    let word_count = word_count.lock().unwrap();
    let mut words = word_count
        .iter()
        .map(|(w, n)| (w.clone(), *n))
        .collect::<Vec<_>>();
    drop(word_count);

    words.sort_by_key(|(_a, n)| *n);

    let n = n.unwrap_or(words.len());

    println!("\n==== Top {n} ===");
    for (word, count) in words.iter().rev().take(n).rev() {
        println!("{word:>30}: {count:<5}");
    }
}

fn is_forbidden_url(url: &Url) -> bool {
    if let Some(host) = url.host_str() {
        for it in FORBIDDEN_HOSTS {
            if host.contains(it) {
                return true;
            }
        }
    }

    let path = url.path();
    for it in FORBIDDEN_PATHS {
        if path.contains(it) {
            return true;
        }
    }

    false
}

// const DEFAULT: &str = "https://idno.se";
const DEFAULT: &str = "https://axis.com";

#[rustfmt::skip]
const FORBIDDEN_HOSTS: &[&str] = &[
    "google",
    "youtube",
    "wikipedia",
    "reddit",
    "yahoo",
    "pinterest",
    "linkedin",
    "facebook",
    // "twitter",
    "x.com",
    "discourse.org",
    "adr",
    "brevo",
    "github",
    "music.apple.com",
    "spotify",
];

#[rustfmt::skip]
const FORBIDDEN_PATHS: &[&str] = &[
    "legal",
    "policy",
    "privacy",
    "guidelines",
    "support",
];

// const DO_WORD_COUNT: bool = true;
const DO_WORD_COUNT: bool = false;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| DEFAULT.into());
    let url = Url::parse(&url).unwrap();

    let _ = std::fs::create_dir("db");

    let (crawler_s, crawler_r) = crossbeam_channel::unbounded();
    let _ = crawler_s.send(url);

    let num_cpus = num_cpus::get_physical();
    // let num_threads = num_cpus * 10;
    // let num_threads = num_cpus * 42;
    // let num_threads = num_cpus * 142;
    let num_threads = num_cpus * 360;

    let working = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let checked = Arc::new(Mutex::new(HashSet::new()));
    let word_count = Arc::new(Mutex::new(HashMap::new()));
    let mut threads = vec![];

    for i in 0..num_threads {
        let checked = Arc::clone(&checked);
        let word_count = Arc::clone(&word_count);
        let working = Arc::clone(&working);
        let crawler_r = crawler_r.clone();
        let crawler_s = crawler_s.clone();

        let thread = std::thread::Builder::new().name(format!("thread-{i}"));
        let it = thread.spawn(move || {
            let mut buffer = Vec::with_capacity(1024 * 5);
            for message in crawler_r.iter() {
                working.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                buffer.clear();
                crawler_work(&message, &checked, &word_count, &mut buffer, &crawler_s);
                working.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        });

        threads.push(it);
    }

    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        report(&word_count, Some(20));

        let n = working.load(std::sync::atomic::Ordering::Relaxed);
        if n == 0 && crawler_r.is_empty() && crawler_s.is_empty() {
            break;
        }
    }

    println!("");
    println!("=============");
    println!("     done!   ");
    println!("=============");
    println!("");
    println!("visited:");

    drop(threads);

    let checked = checked.lock().unwrap();
    for checked in checked.iter() {
        println!("{}", checked.as_str());
    }

    report(&word_count, None);
}
