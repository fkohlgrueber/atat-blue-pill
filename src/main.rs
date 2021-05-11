#![no_main]
#![no_std]

use atat_blue_pill as _; // global logger + panicking-behavior + memory layout

extern crate stm32f1xx_hal as hal;

use hal::{pac::{Peripherals, TIM3, USART2, interrupt}, prelude::*, serial::{Config, Event::Rxne, Rx, Serial}, timer::{CountDownTimer, Event, Timer}};

use atat::{ComQueue, Queues, ResQueue, UrcQueue};

use heapless::{consts, spsc::Queue};
use core::convert::TryInto;


use espresso::{commands::requests, types::{ConnectionStatus, MultiplexingType, WifiMode}};
use no_std_net::{Ipv4Addr, SocketAddr, SocketAddrV4};


static mut INGRESS: Option<atat::IngressManager<consts::U256>> = None;
static mut RX: Option<Rx<USART2>> = None;

static mut G_TIM: Option<CountDownTimer<TIM3>> = None;


#[cortex_m_rt::entry]
fn main() -> ! {
    const SSID: &str = include_str!("../ssid.txt");
    const PW: &str = include_str!("../pw.txt");

    let p = Peripherals::take().unwrap();

    let mut flash = p.FLASH.constrain();
    let mut rcc = p.RCC.constrain();

    let mut gpioa = p.GPIOA.split(&mut rcc.apb2);
    // let mut gpiob = p.GPIOB.split(&mut rcc.ahb2);

    // clock configuration using the default settings (all clocks run at 8 MHz)
    let clocks = rcc.cfgr.freeze(&mut flash.acr);
    // TRY this alternate clock configuration (clocks run at nearly the maximum frequency)
    // let clocks = rcc.cfgr.sysclk(64.mhz()).pclk1(32.mhz()).freeze(&mut flash.acr);

    let mut afio = p.AFIO.constrain(&mut rcc.apb2);

    let tx = gpioa.pa2.into_alternate_push_pull(&mut gpioa.crl);
    let rx = gpioa.pa3;

    let mut timer = Timer::tim3(p.TIM3, &clocks, &mut rcc.apb1).start_count_down(10.hz());
    let at_timer = Timer::tim2(p.TIM2, &clocks, &mut rcc.apb1).start_count_down(100.hz());

    let mut serial = Serial::usart2(
        p.USART2,
        (tx, rx),
        &mut afio.mapr,
        Config::default()
            .baudrate(9_600.bps())
            .parity_none()
            .stopbits(hal::serial::StopBits::STOP1),
        clocks,
        &mut rcc.apb1,
    );

    serial.listen(Rxne);

    static mut RES_QUEUE: ResQueue<consts::U256> = Queue(heapless::i::Queue::u8());
    static mut URC_QUEUE: UrcQueue<consts::U256, consts::U10> = Queue(heapless::i::Queue::u8());
    static mut COM_QUEUE: ComQueue = Queue(heapless::i::Queue::u8());

    let queues = Queues {
        res_queue: unsafe { RES_QUEUE.split() },
        urc_queue: unsafe { URC_QUEUE.split() },
        com_queue: unsafe { COM_QUEUE.split() },
    };

    let (tx, rx) = serial.split();
    // let (mut client, ingress) =
    //     ClientBuilder::new(tx, at_timer, atat::Config::new(atat::Mode::Blocking)).build(queues);
    let (mut client, ingress) = espresso::EspClient::new(tx, at_timer, queues);


    unsafe { INGRESS = Some(ingress) };
    unsafe { RX = Some(rx) };

    // configure NVIC interrupts
    unsafe { cortex_m::peripheral::NVIC::unmask(hal::stm32::Interrupt::TIM3) };
    unsafe { cortex_m::peripheral::NVIC::unmask(hal::stm32::Interrupt::USART2) };
    timer.listen(Event::Update);

    unsafe {
        G_TIM = Some(timer);
    }

    
    defmt::info!("Testing whether device is online… ");
    client.selftest().expect("Self test failed");
    defmt::info!("OK");

    // Get firmware information
    let version = client
        .get_firmware_version()
        .expect("Could not get firmware version");
    defmt::info!("at_version: {}", version.at_version.as_str());
    defmt::info!("sdk_version: {}", version.sdk_version.as_str());
    defmt::info!("compile_time: {}", version.compile_time.as_str());

    // Show current config
    let wifi_mode = client.get_wifi_mode().expect("Could not get wifi mode");
    defmt::info!(
        "Wifi mode:\n  Current: {:?}\n  Default: {:?}",
        wifi_mode.current, wifi_mode.default,
    );

    defmt::info!("Setting current Wifi mode to Station… ");
    client
        .set_wifi_mode(WifiMode::Station, false)
        .expect("Could not set current wifi mode");
    defmt::info!("OK");

    let status = client
        .get_connection_status()
        .expect("Could not get connection status");
    defmt::info!("Connection status: {:?}", status);
    let local_addr = client
        .get_local_address()
        .expect("Could not get local address");
    defmt::info!("Local MAC: {}", defmt::Debug2Format(&local_addr.mac));
    defmt::info!("Local IP:  {}", defmt::Debug2Format(&local_addr.ip));

    match status {
        ConnectionStatus::ConnectedToAccessPoint | ConnectionStatus::TransmissionEnded => {
            defmt::info!("Already connected!");
        }
        _ => {
            defmt::info!("Connecting to access point with SSID {:?}…", SSID);
            let result = client
                .join_access_point(SSID, PW, false)
                .expect("Could not connect to access point");
            defmt::info!("{:?}", result);
            let status = client
                .get_connection_status()
                .expect("Could not get connection status");
            defmt::info!("Connection status: {:?}", status);
        }
    }
    defmt::info!(
        "Local IP: {}",
        defmt::Debug2Format(
            &client
                .get_local_address()
                .expect("Could not get local IP address")
                .ip
        )
    );

    defmt::info!("Creating TCP connection to ipify.com…");
    let remote_ip = Ipv4Addr::new(184, 73, 165, 106);
    let remote_port = 80;
    client
        .send_command(&requests::EstablishConnection::tcp(
            MultiplexingType::NonMultiplexed,
            SocketAddr::V4(SocketAddrV4::new(remote_ip, remote_port)),
        ))
        .expect("Could not establish a TCP connection");
    defmt::info!("Connection established!");
    // defmt::info!("Sending HTTP request…");
    // let data = "GET /?format=text HTTP/1.1\r\nHost: api.ipify.org\r\nUser-Agent: ESP8266\r\n\r\n";
    // client
    //     .send_command(&requests::PrepareSendData::new(
    //         MultiplexingType::NonMultiplexed,
    //         data.len().try_into().unwrap(),
    //     ))
    //     .expect("Could not prepare sending data");
    // client
    //     .send_command(&requests::SendData::<heapless::consts::U72>::new(&data))
    //     .expect("Could not send data");
    client
        .send_command(&requests::CloseConnection::new(
            MultiplexingType::NonMultiplexed,
        ))
        .expect("Could not close connection");
    defmt::info!("Connection closed!");

    defmt::info!("\nStarting main loop, use Ctrl+C to abort…");
    loop {}
}

#[interrupt]
fn TIM3() {
    //defmt::info!("Digest!");
    let ingress = unsafe { INGRESS.as_mut().unwrap() };
    ingress.digest();

    unsafe {
        G_TIM.as_mut().unwrap().wait().unwrap();
    }
}

#[interrupt]
fn USART2() {
    let ingress = unsafe { INGRESS.as_mut().unwrap() };
    let rx = unsafe { RX.as_mut().unwrap() };
    if let Ok(d) = nb::block!(rx.read()) {
        ingress.write(&[d]);
    }
}
