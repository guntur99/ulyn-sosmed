use chrono::{Utc, FixedOffset, DateTime};

pub fn now_jakarta() -> DateTime<FixedOffset> {
    let jakarta_offset = FixedOffset::east_opt(7 * 3600).unwrap();
    Utc::now().with_timezone(&jakarta_offset)
}

pub fn get_topup_success_email(
    name: &str,
    amount: &str,
    credits: &str,
    reference: &str,
    payment_channel: &str,
    lang: Option<&str>,
) -> String {
    let lang_code = lang.unwrap_or("id");
    
    // Map internal tier name to display package name
    let package_display = match credits {
        "basic_pass" | "10" => "Basic Pass",
        "pro_pass" | "60" => "Pro Pass",
        _ => credits,
    };

    let credit_count = match credits {
        "basic_pass" => "10",
        "pro_pass" => "60",
        _ => credits,
    };

    let t = get_email_translations(lang_code, package_display);
    let amount_float = amount.parse::<f64>().unwrap_or(0.0);
    let amount_formatted = format_currency_localized(amount_float, lang_code);
    
    let now = now_jakarta();
    let date_str = now.format("%d %b %Y, %H:%M").to_string();
    let year = now.format("%Y").to_string();

    format!(r##"
<!DOCTYPE html>
<html lang="{lang}">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{subject}</title>
    <link href="https://fonts.googleapis.com/css2?family=Plus+Jakarta+Sans:wght@400;500;600;700;800&display=swap" rel="stylesheet">
    <style>
        body {{ font-family: 'Plus Jakarta Sans', Arial, sans-serif; background-color: #FAFAFA; color: #334155; margin: 0; padding: 0; }}
        .wrapper {{ max-width: 600px; margin: 20px auto; background: #ffffff; border-radius: 20px; overflow: hidden; border: 1px solid #F1F5F9; }}
        .header {{ background: linear-gradient(135deg, #f43f5e 0%, #fb923c 100%); padding: 30px; text-align: center; color: white; }}
        .content {{ padding: 40px; }}
        .greeting {{ font-size: 24px; font-weight: 800; color: #0f172a; margin-bottom: 10px; }}
        .card {{ background: #FFF1F2; border-radius: 15px; padding: 25px; text-align: center; margin: 20px 0; }}
        .btn {{ display: inline-block; background: #f43f5e; color: white !important; padding: 15px 30px; border-radius: 12px; text-decoration: none; font-weight: 800; }}
        .footer {{ padding: 20px; text-align: center; font-size: 12px; color: #94a3b8; }}
    </style>
</head>
<body>
    <div class="wrapper">
        <div class="header">
            <h1 style="margin:0">NGETRIP</h1>
            <p style="margin:5px 0 0 0; opacity:0.8">{brand_slogan}</p>
        </div>
        <div class="content">
            <div class="greeting">{greeting}</div>
            <p>{message}</p>
            <div class="card">
                <div style="font-size:12px; font-weight:800; color:#f43f5e; margin-bottom:5px">{packet_bought}</div>
                <div style="font-size:24px; font-weight:900; color:#0f172a">{package_title}</div>
                <div style="font-size:32px; font-weight:900; color:#f43f5e; margin:10px 0">{amount_formatted}</div>
                <div style="background:#f43f5e; color:white; display:inline-block; padding:5px 15px; border-radius:50px; font-size:12px">+{credit_count} Credits {credits_active}</div>
            </div>
            <table width="100%" style="font-size:14px; border-collapse:collapse">
                <tr><td style="padding:10px 0; color:#64748b">{transaction_no}</td><td align="right" style="font-weight:800">#{reference}</td></tr>
                <tr><td style="padding:10px 0; color:#64748b">{method}</td><td align="right" style="font-weight:800">{payment_channel}</td></tr>
                <tr><td style="padding:10px 0; color:#64748b">{date}</td><td align="right" style="font-weight:800">{date_str}</td></tr>
            </table>
            <div style="text-align:center; margin-top:30px">
                <a href="https://ulyn.pro" class="btn">{cta_button}</a>
            </div>
        </div>
        <div class="footer">
            &copy; {year} NGETRIP • {brand_slogan}
        </div>
    </div>
</body>
</html>
    "##,
    lang = lang_code,
    subject = t.subject,
    greeting = t.greeting.replace("{{name}}", name),
    message = t.message,
    packet_bought = t.packet_bought,
    package_title = t.package_title,
    amount_formatted = amount_formatted,
    credit_count = credit_count,
    credits_active = t.credits_active,
    transaction_no = t.transaction_no,
    reference = reference,
    method = t.method,
    payment_channel = payment_channel,
    date = t.date,
    date_str = date_str,
    cta_button = t.cta_button,
    year = year,
    brand_slogan = t.brand_slogan
    )
}

struct EmailTranslations {
    subject: &'static str,
    greeting: &'static str,
    message: &'static str,
    packet_bought: &'static str,
    package_title: String,
    credits_active: &'static str,
    transaction_no: &'static str,
    method: &'static str,
    date: &'static str,
    cta_button: &'static str,
    brand_slogan: &'static str,
}

fn get_email_translations(lang: &str, package_title: &str) -> EmailTranslations {
    match lang {
        "en" => EmailTranslations {
            subject: "Topup Successful",
            greeting: "Horray, {{name}}! 🚀",
            message: "We have successfully received your payment. Credits have been automatically added to your account.",
            packet_bought: "PACKAGE BOUGHT",
            package_title: package_title.to_string(),
            credits_active: "Active",
            transaction_no: "Transaction No.",
            method: "Method",
            date: "Date",
            cta_button: "START YOUR TRIP NOW! →",
            brand_slogan: "GO EVERYWHERE EASILY!",
        },
        "ja" => EmailTranslations {
            subject: "入金完了",
            greeting: "やったね、{{name}}さん! 🚀",
            message: "お支払いが正常に完了しました。クレジットが自動的にアカウントに追加されました。",
            packet_bought: "購入済みパック",
            package_title: package_title.to_string(),
            credits_active: "有効",
            transaction_no: "取引番号",
            method: "支払い方法",
            date: "日付",
            cta_button: "今すぐ旅行を始める! →",
            brand_slogan: "どこへでも、もっと簡単に!",
        },
        "ko" => EmailTranslations {
            subject: "충전 성공",
            greeting: "축하합니다, {{name}}님! 🚀",
            message: "결제가 성공적으로 완료되었습니다. 크레딧이 계정에 자동으로 추가되었습니다.",
            packet_bought: "구매한 패키지",
            package_title: package_title.to_string(),
            credits_active: "활성",
            transaction_no: "거래 번호",
            method: "결제 방법",
            date: "날짜",
            cta_button: "지금 바로 여행 시작! →",
            brand_slogan: "어디로든 간편하게!",
        },
        "zh" => EmailTranslations {
            subject: "充值成功",
            greeting: "太棒了，{{name}}! 🚀",
            message: "我们已成功收到您的付款。积分已自动添加到您的账户中。",
            packet_bought: "已购买套餐",
            package_title: package_title.to_string(),
            credits_active: "有效",
            transaction_no: "交易编号",
            method: "支付方式",
            date: "日期",
            cta_button: "立即开始旅行! →",
            brand_slogan: "无论去哪，都变简单!",
        },
        "ru" => EmailTranslations {
            subject: "Пополнение успешно",
            greeting: "Ура, {{name}}! 🚀",
            message: "Мы успешно получили ваш платеж. Кредиты были автоматически добавлены на ваш счет.",
            packet_bought: "КУПЛЕННЫЙ ПАКЕТ",
            package_title: package_title.to_string(),
            credits_active: "Активны",
            transaction_no: "№ Транзакции",
            method: "Метод",
            date: "Дата",
            cta_button: "НАЧАТЬ ПУТЕШЕСТВИЕ! →",
            brand_slogan: "ПУТЕШЕСТВОВАТЬ СТАЛО ПРОЩЕ!",
        },
        "nl" => EmailTranslations {
            subject: "Opwaardering geslaagd",
            greeting: "Hoera, {{name}}! 🚀",
            message: "We hebben je betaling succesvol ontvangen. Credits zijn otomatis aan je account toegevoegd.",
            packet_bought: "GEKOCHT PAKKET",
            package_title: package_title.to_string(),
            credits_active: "Actief",
            transaction_no: "Transactie Nr.",
            method: "Methode",
            date: "Datum",
            cta_button: "BEGIN JE REIS NU! →",
            brand_slogan: "OVERAL GEMAKKELIJK NAARTOE!",
        },
        "af" => EmailTranslations {
            subject: "Topup Suksesvol",
            greeting: "Hoera, {{name}}! 🚀",
            message: "Ons het u betaling suksesvol ontvang. Krediete is outomaties by u rekening gevoeg.",
            packet_bought: "PAKKET GEKOOP",
            package_title: package_title.to_string(),
            credits_active: "Aktief",
            transaction_no: "Transaksie Nr.",
            method: "Metode",
            date: "Datum",
            cta_button: "BEGIN NOU JOU REIS! →",
            brand_slogan: "GAAN ORAL MAKLIKER!",
        },
        "ar" => EmailTranslations {
            subject: "تم الشحن بنجاح",
            greeting: "مرحباً، {{name}}! 🚀",
            message: "لقد استلمنا دفعتك بنجاح. تمت إضافة الرصيد إلى حسابك تلقائيًا.",
            packet_bought: "الباقة المشتراة",
            package_title: package_title.to_string(),
            credits_active: "نشط",
            transaction_no: "رقم العملية",
            method: "الطريقة",
            date: "التاريخ",
            cta_button: "ابدأ رحلتك الآن! →",
            brand_slogan: "الذهاب لأي مكان أصبح سهلاً!",
        },
        _ => EmailTranslations {
            subject: "Topup Berhasil",
            greeting: "Horray, {{name}}! 🚀",
            message: "Pembayaran kamu sudah berhasil kami terima. Credits telah ditambahkan ke akunmu secara otomatis.",
            packet_bought: "PAKET DIBELI",
            package_title: package_title.to_string(),
            credits_active: "Aktif",
            transaction_no: "No. Transaksi",
            method: "Metode",
            date: "Tanggal",
            cta_button: "GAS LIBURAN SEKARANG! →",
            brand_slogan: "KEMANAPUN JADI GAMPANG PISAN!",
        },
    }
}

fn format_currency_localized(amount: f64, lang: &str) -> String {
    let amount_int = amount as i64;
    match lang {
        "ja" => match amount_int {
            15000 => "¥150".to_string(),
            49000 => "¥500".to_string(),
            _ => format!("¥{}", amount_int / 100)
        },
        "en" => match amount_int {
            15000 => "$0.99".to_string(),
            49000 => "$3.99".to_string(),
            _ => format!("${:.2}", amount / 15000.0)
        },
        "ko" => match amount_int {
            15000 => "₩1,300".to_string(),
            49000 => "₩4,500".to_string(),
            _ => format!("₩{}", amount_int / 10)
        },
        "zh" => match amount_int {
            15000 => "¥7".to_string(),
            49000 => "¥22".to_string(),
            _ => format!("¥{}", (amount / 2000.0) as i64)
        },
        "ru" => match amount_int {
            15000 => "100 ₽".to_string(),
            49000 => "350 ₽".to_string(),
            _ => format!("{} ₽", (amount / 150.0) as i64)
        },
        "nl" => match amount_int {
            15000 => "€1,00".to_string(),
            49000 => "€3,50".to_string(),
            _ => format!("€{:.2}", amount / 15000.0).replace(".", ",")
        },
        "af" => match amount_int {
            15000 => "R20".to_string(),
            49000 => "R75".to_string(),
            _ => format!("R{}", (amount / 750.0) as i64)
        },
        "ar" => match amount_int {
            15000 => "٤ ر.س".to_string(),
            49000 => "١٥ ر.س".to_string(),
            _ => format!("{} SAR", (amount / 4000.0) as i64)
        },
        _ => match amount_int {
            15000 => "Rp 15rb".to_string(),
            49000 => "Rp 49rb".to_string(),
            _ => format!("Rp {}", amount_int)
        },
    }
}
