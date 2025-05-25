use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use http_req::request;
use tl::VDom;
use url::Url;

const DEFAULT: &str = "https://doc.rust-lang.org/";

fn is_forbidden_url(url: &Url) -> bool {
    if let Some(host) = url.host_str() {
        if host.contains("google") {
            return true;
        }
        if host.contains("youtube") {
            return true;
        }
    }

    false
}

fn crawler_work(
    message: &Url,
    checked: &Arc<Mutex<HashSet<Url>>>,
    buffer: &mut Vec<u8>,
    to_crawl: &crossbeam_channel::Sender<Url>,
) {
    let url = message;

    if is_forbidden_url(&url) {
        return;
    }

    let mut checked = checked.lock().unwrap();
    if !checked.insert(url.clone()) {
        return;
    }
    drop(checked);
    let Ok(res) = request::get(&url, buffer) else {
        return;
    };

    let status = res.status_code();
    let reason = res.reason();

    let body = unsafe { std::str::from_utf8_unchecked(&buffer) };

    let id = std::thread::current().id();
    println!("{id:?} {status}: {reason} {}", url.as_str());

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
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| DEFAULT.into());
    let url = Url::parse(&url).unwrap();

    let (crawler_s, crawler_r) = crossbeam_channel::unbounded();
    let _ = crawler_s.send(url);

    let num_cpus = num_cpus::get_physical();
    let num_threads = num_cpus * 40;

    let working = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let checked = Arc::new(Mutex::new(HashSet::new()));
    let mut threads = vec![];

    for _ in 0..num_threads {
        let checked = Arc::clone(&checked);
        let working = Arc::clone(&working);
        let crawler_r = crawler_r.clone();
        let crawler_s = crawler_s.clone();
        threads.push(std::thread::spawn(move || {
            let mut buffer = Vec::with_capacity(1024 * 5);
            for message in crawler_r.iter() {
                working.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                crawler_work(&message, &checked, &mut buffer, &crawler_s);
                working.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }));
    }

    loop {
        let n = working.load(std::sync::atomic::Ordering::Relaxed);
        if n != 0 {
            continue;
        }

        if crawler_r.is_empty() && crawler_s.is_empty() {
            break;
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
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
}
