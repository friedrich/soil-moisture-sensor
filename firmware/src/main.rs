mod arr_deque;

use crate::arr_deque::ArrDeque;
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use embedded_svc;
use embedded_svc::http::Method;
use embedded_svc::io::Write;
use esp_idf_hal::units::FromValueType;
use esp_idf_hal::{adc, gpio, ledc, reset};
use esp_idf_hal::{delay::FreeRtos, peripherals};
use esp_idf_svc::netif::IpEvent;
use esp_idf_svc::wifi::{EspWifi, WifiEvent};
use esp_idf_svc::{eventloop, nvs, sntp};
use std::sync::mpsc::channel;
use std::time::Duration;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

const WRITE_URL: &str = env!("WRITE_URL");
const AUTHORIZATION: &str = env!("AUTHORIZATION");
const LINE_PREFIX: &str = env!("LINE_PREFIX");

const MEASUREMENT_INTERVAL: Duration = Duration::from_secs(3600);
const MIN_RECORDED_MEASUREMENTS: usize = 6;
const MAX_RECORDED_MEASUREMENTS: usize = 1000;

#[derive(Clone)]
struct Measurement {
    value: u16,
    time: u32,
}

#[link_section = ".rtc.data.rtc_memory"]
static mut MEASUREMENTS: ArrDeque<Measurement, MAX_RECORDED_MEASUREMENTS> = ArrDeque::new();

fn main() -> Result<()> {
    esp_idf_sys::link_patches();

    if let Err(e) = run() {
        println!("error: {}", e);
    }

    unsafe {
        go_to_sleep();
    }
}

fn run() -> Result<()> {
    let peripherals = peripherals::Peripherals::take().context("peripherals already taken")?;
    let mut led_driver = gpio::PinDriver::output(peripherals.pins.gpio7)?;

    let mut power_mode_driver = gpio::PinDriver::output(peripherals.pins.gpio10)?;
    power_mode_driver.set_high()?;

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

    let clock_source = unsafe { esp_idf_sys::rtc_clk_slow_freq_get() };
    if clock_source != esp_idf_sys::rtc_slow_freq_t_RTC_SLOW_FREQ_32K_XTAL {
        bail!("wrong slow clock source");
    }

    if reset::ResetReason::get() != reset::ResetReason::DeepSleep {
        greeting(&mut led_driver)?;
    } else {
        led_driver.set_high()?;
    }

    sensor_pwm_driver.set_duty(sensor_pwm_driver.get_max_duty() / 100)?;
    FreeRtos::delay_ms(20); // TODO: good value?

    match adc_driver.read(&mut adc_channel_driver) {
        Ok(value) => {
            let time = slow_clock_seconds();
            println!("recorded value: {} at {}", value, time);

            unsafe {
                MEASUREMENTS.overwriting_push_back(Measurement { value, time });
                if MEASUREMENTS.len() < MIN_RECORDED_MEASUREMENTS {
                    return Ok(());
                }
            }
        }
        Err(e) => {
            bail!("error measuring: {}", e);
        }
    };

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

    let (wifi_started_tx, wifi_started_rx) = channel();
    let (wifi_connected_tx, wifi_connected_rx) = channel();
    let _wifi_subscription = sysloop.subscribe(move |event: &WifiEvent| match event {
        WifiEvent::StaStarted => {
            let _ = wifi_started_tx.send(());
        }
        WifiEvent::StaConnected => {
            let _ = wifi_connected_tx.send(Ok(()));
        }
        WifiEvent::StaDisconnected => {
            let _ = wifi_connected_tx.send(Err(anyhow!("WiFi disconnected")));
        }
        _ => {}
    })?;

    let (ip_assigned_tx, ip_assigned_rx) = channel();
    let _netif_subscription = sysloop.subscribe(move |event: &IpEvent| match event {
        IpEvent::DhcpIpAssigned(_) | IpEvent::DhcpIp6Assigned(_) => {
            let _ = ip_assigned_tx.send(());
        }
        _ => {}
    })?;

    esp_wifi.start()?;

    wifi_started_rx.recv()?;
    println!("connecting WiFi...");
    esp_wifi.connect()?;

    wifi_connected_rx.recv()??;
    println!("WiFi connected.");

    ip_assigned_rx.recv()?;
    println!("IP address obtained, syncing time....");

    let sntp = sntp::EspSntp::new_default()?;
    while sntp.get_sync_status() != sntp::SyncStatus::Completed {
        FreeRtos::delay_ms(100);
    }
    println!("time synced, sending data..");

    let time_offset = Utc::now().timestamp() - slow_clock_seconds() as i64;

    let measurements: Vec<_> = unsafe { MEASUREMENTS.iter().cloned().collect() };
    send_values(measurements.as_slice(), time_offset)?;
    println!("successfully sent data.");

    unsafe {
        MEASUREMENTS = ArrDeque::new();
    }

    Ok(())
}

fn slow_clock_seconds() -> u32 {
    let rtc_time = unsafe { esp_idf_sys::rtc_time_get() };
    (rtc_time / u64::from(esp_idf_sys::RTC_SLOW_CLK_FREQ_32K)) as _
}

fn greeting<T: gpio::Pin, MODE: gpio::OutputMode>(
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

unsafe fn go_to_sleep() -> ! {
    let delay = MEASUREMENT_INTERVAL.as_micros() as _;
    esp_idf_sys::esp_sleep_enable_timer_wakeup(delay);
    esp_idf_sys::esp_deep_sleep_start();
    unreachable!();
}

fn send_values(measurements: &[Measurement], time_offset: i64) -> anyhow::Result<()> {
    let http_client_config = esp_idf_svc::http::client::Configuration {
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    let data: String = measurements
        .iter()
        .map(|m| {
            format!(
                "{}{} {}000000000\n",
                LINE_PREFIX,
                m.value,
                m.time as i64 + time_offset
            )
        })
        .collect();

    println!("{}", data);

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
