use std::{env, sync::Arc};
// use std::time::{Instant, Duration};

use mysql_async::{params, Conn, prelude::Queryable};

use once_cell::sync::Lazy;

use ingress_intel_rs::Intel;

use reqwest::{Url, cookie::CookieStore, ClientBuilder, cookie::Jar};

// use tokio::timer::delay;

use log::{error, info};

static DATABASE_URL: Lazy<String> = Lazy::new(|| env::var("DATABASE_URL").expect("Missing DATABASE_URL env var"));
static USERNAME: Lazy<Option<String>> = Lazy::new(|| env::var("USERNAME").ok());
static PASSWORD: Lazy<Option<String>> = Lazy::new(|| env::var("PASSWORD").ok());
static COOKIES: Lazy<Option<String>> = Lazy::new(|| env::var("COOKIES").ok());
static RDM_URL: Lazy<Option<String>> = Lazy::new(|| env::var("RDM_URL").ok());
static RDM_USERNAME: Lazy<Option<String>> = Lazy::new(|| env::var("RDM_USERNAME").ok());
static RDM_PASSWORD: Lazy<Option<String>> = Lazy::new(|| env::var("RDM_PASSWORD").ok());

#[tokio::main]
async fn main() -> Result<(), ()> {
    env_logger::init();

    let mut conn = Conn::new(DATABASE_URL.as_str()).await
        .map_err(|e| error!("MySQL connection error: {}", e))?;

    let ids: Vec<String> = conn.query_iter("SELECT pokestop.id FROM pokestop INNER JOIN gym ON pokestop.id = gym.id AND gym.name IS NULL").await
        .map_err(|e| error!("MySQL new gyms select query error: {}", e))?
        .collect_and_drop().await
        .map_err(|e| error!("MySQL new gyms collect query error: {}", e))?;

    if !ids.is_empty() {
        info!("Upgrading {} pokestops to gyms", ids.len());

        let query = format!("UPDATE gym INNER JOIN pokestop ON gym.id = pokestop.id SET gym.name=pokestop.name, gym.url=pokestop.url WHERE gym.id IN ('{}')", ids.join("', '"));
        conn.query_drop(&query).await
            .map_err(|e| error!("MySQL gym update query error: {}", e))?;
        conn.query_drop("DELETE pokestop FROM pokestop INNER JOIN gym ON pokestop.id = gym.id WHERE pokestop.id IS NOT NULL").await
            .map_err(|e| error!("MySQL delete pokestops query error: {}", e))?;
    }

    let ids: Vec<String> = conn.query_iter("SELECT gym.id FROM gym INNER JOIN pokestop ON gym.id = pokestop.id AND pokestop.name IS NULL").await
        .map_err(|e| error!("MySQL new pokestops select query error: {}", e))?
        .collect_and_drop().await
        .map_err(|e| error!("MySQL new pokestops collect query error: {}", e))?;

    if !ids.is_empty() {
        info!("Downgrading {} gyms to pokestops", ids.len());

        let query = format!("UPDATE pokestop INNER JOIN gym ON pokestop.id = gym.id SET pokestop.name=gym.name, pokestop.url=gym.url WHERE pokestop.id IN ('{}')", ids.join("', '"));
        conn.query_drop(&query).await
            .map_err(|e| error!("MySQL pokestops update query error: {}", e))?;
        conn.query_drop("DELETE gym FROM gym INNER JOIN pokestop ON gym.id = pokestop.id WHERE gym.id IS NOT NULL").await
            .map_err(|e| error!("MySQL delete gyms query error: {}", e))?;
    }

    let mut intel = Intel::build(USERNAME.as_ref().map(|s| s.as_str()), PASSWORD.as_ref().map(|s| s.as_str()));

    if let Some(cookies) = &*COOKIES {
        for cookie in cookies.split("; ") {
            if let Some((pos, _)) = cookie.match_indices('=').next() {
                intel.add_cookie(&cookie[0..pos], &cookie[(pos + 1)..]);
            }
        }
    }

    let ids: Vec<String> = conn.query_iter("SELECT id FROM pokestop WHERE name IS NULL").await
        .map_err(|e| error!("MySQL pokestop select query error: {}", e))?
        .collect_and_drop().await
        .map_err(|e| error!("MySQL pokestop collect query error: {}", e))?;

    info!("Found {} unnamed pokestops", ids.len());

    for id in ids {
        match intel.get_portal_details(&id).await {
            Ok(details) => {
                conn.exec_drop("UPDATE pokestop SET name = :name, url = :url WHERE id = :id", params! {
                        "id" => id,
                        "name" => details.result.get_name().to_owned(),
                        "url" => details.result.get_url().to_owned(),
                    }).await
                    .map_err(|e| error!("MySQL pokestop update query error: {}", e))?;
            },
            Err(_) => error!("Error reading pokestop \"{}\" details", id),
        }

        // wait 1 second between requests
        // delay(Instant::now() + Duration::from_secs(1)).await;
    }

    let ids: Vec<String> = conn.query_iter("SELECT id FROM gym WHERE name IS NULL").await
        .map_err(|e| error!("MySQL gym select query error: {}", e))?
        .collect_and_drop().await
        .map_err(|e| error!("MySQL gym collect query error: {}", e))?;

    info!("Found {} unnamed gyms", ids.len());

    for id in ids {
        match intel.get_portal_details(&id).await {
            Ok(details) => {
                conn.exec_drop("UPDATE gym SET name = :name, url = :url WHERE id = :id", params! {
                        "id" => id,
                        "name" => details.result.get_name().to_owned(),
                        "url" => details.result.get_url().to_owned(),
                    }).await
                    .map_err(|e| error!("MySQL gym update query error: {}", e))?;
            },
            Err(_) => error!("Error reading gym \"{}\" details", id),
        }

        // wait 1 second between requests
        // delay(Instant::now() + Duration::from_secs(1)).await;
    }

    if let Some(url) = RDM_URL.as_ref() {
        // start a new client with cookie jar
        let jar = Arc::new(Jar::default());
        let client = ClientBuilder::new()
            .cookie_store(true)
            .cookie_provider(Arc::clone(&jar))
            .build()
            .map_err(|e| error!("RDM client build error: {}", e))?;
        // first call login to get CSRF-TOKEN
        let login_url = Url::parse(&format!("{}/login", url))
            .map_err(|e| error!("RDM URL parse error: {}", e))?;
        client.get(login_url.clone())
            .send()
            .await
            .map_err(|e| error!("RDM get login error: {}", e))?
            .error_for_status()
            .map_err(|e| error!("RDM get login failed: {}", e))?;
        // extract CSRF-TOKEN
        let csrf_token = jar.cookies(&login_url)
            .and_then(|s| {
                s.to_str()
                    .ok()
                    .and_then(|s| {
                        s.split(';')
                            .map(|ss| ss.split('=').collect::<Vec<_>>())
                            .find(|sv| (&sv[0]).trim().eq_ignore_ascii_case("CSRF-TOKEN"))
                            .map(|mut sv| sv.remove(1))
                    })
                    .map(|s| s.to_owned())
            });
        if csrf_token.is_none() {
            error!("Can't find CSRF-TOKEN header");
            return Err(());
        }
        // login
        client.post(login_url.clone())
            .header("Referer", login_url.to_string())
            .form(&vec![
                ("username-email", RDM_USERNAME.as_deref()),
                ("password", RDM_PASSWORD.as_deref()),
                ("_csrf", csrf_token.as_deref())
            ])
            .send()
            .await
            .map_err(|e| error!("RDM post login error: {}", e))?
            .error_for_status()
            .map_err(|e| error!("RDM post login failed: {}", e))?;
        // clear cache
        let dashboard_url = Url::parse(&format!("{}/dashboard/utilities", url))
            .map_err(|e| error!("RDM URL parse error: {}", e))?;
        client.post(dashboard_url.clone())
            .header("Referer", dashboard_url.to_string())
            .form(&vec![
                ("action", Some("clear_memcache")),
                ("_csrf", csrf_token.as_deref())
            ])
            .send()
            .await
            .map_err(|e| error!("RDM clear cache error: {}", e))?
            .error_for_status()
            .map_err(|e| error!("RDM clear cache failed: {}", e))?;
    }

    Ok(())
}
