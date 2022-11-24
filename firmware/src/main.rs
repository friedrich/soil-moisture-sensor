use anyhow::{bail, Context, Result};
use embedded_svc;
use embedded_svc::http::Method;
use embedded_svc::io::Write;
use esp_idf_hal::units::FromValueType;
use esp_idf_hal::{adc, gpio, ledc};
use esp_idf_hal::{delay::FreeRtos, peripherals};
use esp_idf_svc::netif::IpEvent;
use esp_idf_svc::wifi::{EspWifi, WifiEvent};
use esp_idf_svc::{eventloop, nvs};
use std::sync::mpsc::channel;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

const WRITE_URL: &str = env!("WRITE_URL");
const AUTHORIZATION: &str = env!("AUTHORIZATION");
const LINE_PREFIX: &str = env!("LINE_PREFIX");

fn main() -> Result<()> {
    esp_idf_sys::link_patches();

    let peripherals = peripherals::Peripherals::take().context("peripherals already taken")?;
    let mut led_driver = gpio::PinDriver::output(peripherals.pins.gpio7)?;

    let mut power_mode_driver = gpio::PinDriver::output(peripherals.pins.gpio10)?;
    power_mode_driver.set_high()?;

    early_greeting(&mut led_driver)?;

    let mut adc_driver = adc::AdcDriver::new(
        peripherals.adc1,
        &adc::config::Config::new().calibration(true),
    )?;
    let mut adc_channel_driver: adc::AdcChannelDriver<gpio::Gpio4, adc::Atten11dB<_>> =
        adc::AdcChannelDriver::new(peripherals.pins.gpio4)?;

    let pwm_config = ledc::config::TimerConfig::new().frequency(50.kHz().into());
    let mut sensor_pwm_driver = ledc::LedcDriver::new(
        peripherals.ledc.channel0,
        ledc::LedcTimerDriver::new(peripherals.ledc.timer0, &pwm_config)?,
        peripherals.pins.gpio5,
        &pwm_config,
    )?;

    sensor_pwm_driver.set_duty(sensor_pwm_driver.get_max_duty() / 100)?;

    let sysloop = eventloop::EspSystemEventLoop::take()?;
    let nvs_partition = nvs::EspDefaultNvsPartition::take()?;

    let mut esp_wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs_partition))?;
    esp_wifi.set_configuration(&embedded_svc::wifi::Configuration::Client(
        embedded_svc::wifi::ClientConfiguration {
            ssid: WIFI_SSID.into(),
            password: WIFI_PASSWORD.into(),
            channel: None,
            ..Default::default()
        },
    ))?;

    let (wifi_tx, wifi_rx) = channel();
    let _wifi_subscription = sysloop.subscribe(move |event: &WifiEvent| match event {
        WifiEvent::StaStarted => {
            let _ = wifi_tx.send(());
        }
        WifiEvent::StaConnected => {
            println!("WiFi connected.");
        }
        WifiEvent::StaDisconnected => {
            println!("WiFi disconnected.");
            go_to_sleep();
        }
        _ => {}
    })?;

    let (ip_tx, ip_rx) = channel();
    let _netif_subscription = sysloop.subscribe(move |event: &IpEvent| match event {
        IpEvent::DhcpIpAssigned(_) | IpEvent::DhcpIp6Assigned(_) => {
            let _ = ip_tx.send(());
        }
        _ => {}
    })?;

    esp_wifi.start()?;

    wifi_rx.recv()?;
    println!("connecting WiFi...");
    if let Err(e) = esp_wifi.connect() {
        println!("error connecting to WiFi: {}", e);
    }

    ip_rx.recv()?;
    println!("IP address obtained, sending data...");
    match adc_driver.read(&mut adc_channel_driver) {
        Ok(dryness) => {
            if let Err(e) = send_value(dryness) {
                println!("error sending data: {}", e);
            } else {
                println!("successfully sent data.")
            }
        }
        Err(e) => {
            println!("error measuring: {}", e);
        }
    }

    go_to_sleep();
}

fn early_greeting<T: gpio::Pin, MODE: gpio::OutputMode>(
    led_pin: &mut gpio::PinDriver<T, MODE>,
) -> Result<()> {
    for _ in 0..4 {
        led_pin.set_low()?;
        FreeRtos::delay_ms(20);
        led_pin.set_high()?;
        FreeRtos::delay_ms(100);
    }
    FreeRtos::delay_ms(400);
    led_pin.set_low()?;
    FreeRtos::delay_ms(1000);
    led_pin.set_high()?;
    FreeRtos::delay_ms(500);

    Ok(())
}

fn go_to_sleep() -> ! {
    let delay_in_ms = 10 * 60 * 1000;
    unsafe {
        esp_idf_sys::esp_sleep_enable_timer_wakeup(delay_in_ms * 1000);
        esp_idf_sys::esp_deep_sleep_start();
        unreachable!();
    }
}

fn send_value(dryness: u16) -> anyhow::Result<()> {
    let http_client_config = esp_idf_svc::http::client::Configuration {
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    let data = format!("{}{}", LINE_PREFIX, dryness);

    let content_length = data.len().to_string();
    let headers = vec![
        ("Authorization", AUTHORIZATION),
        ("Content-Length", &content_length),
    ];

    let mut http_client = esp_idf_svc::http::client::EspHttpConnection::new(&http_client_config)?;
    http_client.initiate_request(Method::Post, WRITE_URL, &headers)?;
    http_client.write_all(data.as_bytes())?;
    http_client.initiate_response()?;

    let status = http_client.status();
    if status < 200 || status >= 300 {
        let mut response = vec![0; 1000];
        http_client.read(&mut response)?;
        bail!(
            "HTTP status {}: {}",
            status,
            String::from_utf8_lossy(&response)
        );
    }

    Ok(())
}
