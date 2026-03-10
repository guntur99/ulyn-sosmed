use serde::Serialize;
use std::env;
use reqwest::Client;

#[derive(Serialize)]
struct MailtrapAddress {
    email: String,
    name: Option<String>,
}

#[derive(Serialize)]
struct MailtrapPayload {
    from: MailtrapAddress,
    to: Vec<MailtrapAddress>,
    subject: String,
    html: String,
    category: Option<String>,
}

pub async fn send_html_email(_state: Option<&crate::AppState>, to: &str, subject: &str, html_content: &str) -> Result<(), String> {
    // Check if we should use API (Mailtrap)
    if let Ok(api_token) = env::var("MAILTRAP_API_TOKEN") {
        if !api_token.is_empty() {
             match send_via_api(to, subject, html_content, &api_token).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    tracing::warn!("Mailtrap API failed: {}. Falling back to SMTP...", e);
                }
            }
        }
    }

     // Fallback to SMTP
    send_via_smtp(to, subject, html_content).await
}

async fn send_via_api(to: &str, subject: &str, html_content: &str, api_token: &str) -> Result<(), String> {
    tracing::info!("Attempting to send email via Mailtrap API to {}", to);
    let from_email = env::var("MAIL_FROM_ADDRESS").unwrap_or_else(|_| "team@ulyn.fun".to_string());
    let from_name = env::var("MAIL_FROM_NAME").unwrap_or_else(|_| "Ulyn AI".to_string());

    let payload = MailtrapPayload {
        from: MailtrapAddress {
            email: from_email,
            name: Some(from_name),
        },
        to: vec![MailtrapAddress {
            email: to.to_string(),
            name: None,
        }],
        subject: subject.to_string(),
        html: html_content.to_string(),
        category: Some("Transaction".to_string()),
    };

    let client = Client::new();
    let response = client.post("https://send.api.mailtrap.io/api/send")
        .header("Authorization", format!("Bearer {}", api_token))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Mailtrap API request failed: {}", e);
            format!("Request failed: {}", e)
        })?;

    if response.status().is_success() {
        tracing::info!("Mailtrap API: Email sent successfully to {}", to);
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        tracing::error!("Mailtrap API Error response: {}", body);
        Err(format!("Mailtrap API Error: {}", body))
    }
}

async fn send_via_smtp(to: &str, subject: &str, html_content: &str) -> Result<(), String> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    tracing::info!("Attempting to send email via SMTP to {}", to);
    let server = env::var("MAIL_HOST").map_err(|_| "MAIL_HOST not set")?;
    let port = env::var("MAIL_PORT").unwrap_or_else(|_| "587".to_string()).parse::<u16>().unwrap_or(587);
    let username = env::var("MAIL_USERNAME").map_err(|_| "MAIL_USERNAME not set")?;
    let password = env::var("MAIL_PASSWORD").map_err(|_| "MAIL_PASSWORD not set")?;
    let from_email = env::var("MAIL_FROM_ADDRESS").unwrap_or_else(|_| "team@ulyn.fun".to_string());
    let from_name = env::var("MAIL_FROM_NAME").unwrap_or_else(|_| "Ulyn AI".to_string());

    tracing::info!("SMTP Config: host={}, port={}, user={}, from={}", server, port, username, from_email);

    let email = Message::builder()
        .from(format!("{} <{}>", from_name, from_email).parse().map_err(|e: lettre::address::AddressError| e.to_string())?)
        .to(to.parse().map_err(|e: lettre::address::AddressError| e.to_string())?)
        .subject(subject)
        .header(lettre::message::header::ContentType::TEXT_HTML)
        .body(html_content.to_string())
        .map_err(|e| {
            tracing::error!("Failed to build email message: {}", e);
            e.to_string()
        })?;

    let creds = Credentials::new(username, password);
    let mailer: AsyncSmtpTransport<Tokio1Executor> = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&server)
        .map_err(|e| {
            tracing::error!("Failed to create SMTP transport: {}", e);
            e.to_string()
        })?
        .port(port)
        .credentials(creds)
        .build();

    match mailer.send(email).await {
        Ok(_) => {
            tracing::info!("SMTP: Email sent successfully to {}", to);
            Ok(())
        }
        Err(e) => {
            tracing::error!("SMTP: Failed to send email to {}: {}", to, e);
            Err(e.to_string())
        }
    }
}
