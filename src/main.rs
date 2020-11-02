use std::env;
// use std::time::{Instant, Duration};

use mysql_async::{params, Conn, prelude::Queryable};

use once_cell::sync::Lazy;

use ingress_intel_rs::Intel;

// use tokio::timer::delay;

use log::{error, info};

static DATABASE_URL: Lazy<String> = Lazy::new(|| env::var("DATABASE_URL").expect("Missing DATABASE_URL env var"));
static USERNAME: Lazy<Option<String>> = Lazy::new(|| env::var("USERNAME").ok());
static PASSWORD: Lazy<Option<String>> = Lazy::new(|| env::var("PASSWORD").ok());
static COOKIES: Lazy<Option<String>> = Lazy::new(|| env::var("COOKIES").ok());

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

    Ok(())
}
