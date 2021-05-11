#![no_main]
#![no_std]

use atat_blue_pill as _; // global logger + panicking-behavior + memory layout

extern crate stm32f1xx_hal as hal;

//mod common;

use cortex_m::asm;
use hal::{pac::{Peripherals, TIM3, USART2, interrupt}, prelude::*, serial::{Config, Event::Rxne, Rx, Serial}, timer::{CountDownTimer, Event, Timer}};

use atat::{AtatClient, AtatCmd, ClientBuilder, ComQueue, DefaultDigester, DefaultUrcMatcher, Error, GenericError, InternalError, Queues, ResQueue, UrcQueue};
use atat::atat_derive::{AtatCmd, AtatResp};

use heapless::{Vec, consts, spsc::Queue};


#[derive(Clone, AtatResp)]
pub struct NoResponse;

#[derive(Clone, AtatCmd)]
#[at_cmd("", NoResponse, timeout_ms = 1000)]
pub struct At;

// impl AtatCmd for At {
//     type CommandLen = heapless::consts::U4;
//     type Response = NoResponse;
//     type Error = GenericError;

//     fn as_bytes(&self) -> Vec<u8, Self::CommandLen> {
//         Vec::from_slice(b"AT\r\n").unwrap()
//     }

//     fn parse(&self, resp: Result<&[u8], &InternalError>) -> Result<Self::Response, Error<Self::Error>> {
//         let resp = core::str::from_utf8(resp?).unwrap();
//         if !resp.trim().is_empty() {
//             Err(atat::Error::InvalidResponse)
//         } else {
//             Ok(NoResponse)
//         }
//     }
// }


static mut INGRESS: Option<atat::IngressManager<consts::U256>> = None;
static mut RX: Option<Rx<USART2>> = None;

static mut G_TIM: Option<CountDownTimer<TIM3>> = None;


#[cortex_m_rt::entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();

    let mut flash = p.FLASH.constrain();
    let mut rcc = p.RCC.constrain();
    let mut pwr = p.PWR;

    let mut gpioa = p.GPIOA.split(&mut rcc.apb2);
    // let mut gpiob = p.GPIOB.split(&mut rcc.ahb2);

    // clock configuration using the default settings (all clocks run at 8 MHz)
    let clocks = rcc.cfgr.freeze(&mut flash.acr);
    // TRY this alternate clock configuration (clocks run at nearly the maximum frequency)
    // let clocks = rcc.cfgr.sysclk(64.mhz()).pclk1(32.mhz()).freeze(&mut flash.acr);

    let mut afio = p.AFIO.constrain(&mut rcc.apb2);

    let tx = gpioa.pa2.into_alternate_push_pull(&mut gpioa.crl);
    let rx = gpioa.pa3;

    let mut timer = Timer::tim3(p.TIM3, &clocks, &mut rcc.apb1).start_count_down(1.hz());
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
    let (mut client, ingress) =
        ClientBuilder::new(tx, at_timer, atat::Config::new(atat::Mode::Blocking)).build(queues);

    unsafe { INGRESS = Some(ingress) };
    unsafe { RX = Some(rx) };

    // configure NVIC interrupts
    unsafe { cortex_m::peripheral::NVIC::unmask(hal::stm32::Interrupt::TIM3) };
    unsafe { cortex_m::peripheral::NVIC::unmask(hal::stm32::Interrupt::USART2) };
    timer.listen(Event::Update);

    unsafe {
        G_TIM = Some(timer);
    }

    // if all goes well you should reach this breakpoint
    //asm::bkpt();

    loop {
        //asm::wfi();

        match client.send(&At) {
            Ok(response) => {
                defmt::info!("Got OK");
                // Do something with response here
            }
            Err(e) => {
                defmt::info!("Got Err")
            }
        }
    }
}

#[interrupt]
fn TIM3() {
    defmt::info!("Digest!");
    let ingress = unsafe { INGRESS.as_mut().unwrap() };
    ingress.digest();

    unsafe {
        G_TIM.as_mut().unwrap().wait();
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
