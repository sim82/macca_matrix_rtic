#![no_main]
#![no_std]

extern crate panic_halt;

extern crate stm32l4xx_hal as hal;
use mocca_matrix_rtic::prelude::*;
use smart_leds::{brightness, RGB8};
use ws2812::Ws2812;

use core::fmt::Write;
use embedded_graphics::{fonts, pixelcolor, prelude::*, style};
use hal::{
    device::I2C1,
    gpio::gpioa::PA0,
    gpio::{
        Alternate, Edge, Floating, Input, OpenDrain, Output, PullUp, PushPull, PA1, PA5, PA6, PA7,
        PB6, PB7, PB8, PB9,
    },
    i2c::I2c,
    prelude::*,
    spi::Spi,
    stm32,
    timer::{Event, Timer},
};
use hal::{
    gpio::PC13,
    stm32l4::stm32l4x2::{interrupt, Interrupt, NVIC},
};
use heapless::consts::*;
use heapless::String;
use rtic::cyccnt::U32Ext;
use smart_leds::SmartLedsWrite;
use ssd1306::{mode::GraphicsMode, prelude::*, Builder, I2CDIBuilder};
use ws2812_spi as ws2812;

const GAMMA8: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 5, 5, 5,
    5, 6, 6, 6, 6, 7, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10, 10, 11, 11, 11, 12, 12, 13, 13, 13, 14,
    14, 15, 15, 16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 24, 24, 25, 25, 26, 27,
    27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 35, 35, 36, 37, 38, 39, 39, 40, 41, 42, 43, 44, 45, 46,
    47, 48, 49, 50, 50, 51, 52, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 66, 67, 68, 69, 70, 72,
    73, 74, 75, 77, 78, 79, 81, 82, 83, 85, 86, 87, 89, 90, 92, 93, 95, 96, 98, 99, 101, 102, 104,
    105, 107, 109, 110, 112, 114, 115, 117, 119, 120, 122, 124, 126, 127, 129, 131, 133, 135, 137,
    138, 140, 142, 144, 146, 148, 150, 152, 154, 156, 158, 160, 162, 164, 167, 169, 171, 173, 175,
    177, 180, 182, 184, 186, 189, 191, 193, 196, 198, 200, 203, 205, 208, 210, 213, 215, 218, 220,
    223, 225, 228, 231, 233, 236, 239, 241, 244, 247, 249, 252, 255,
];

const REFRESH_DISPLAY_PERIOD: u32 = 64_000_000 / 40;
const REFRESH_LED_STRIP_PERIOD: u32 = 64_000_000 / 32;

const NUM_LEDS: usize = 291;

mod power_zones {
    use super::NUM_LEDS;

    const START0: usize = 0;
    const SIZE0: usize = 8 + 9 + 10 + 11 + 15 + 16 + 17;
    const START1: usize = SIZE0;
    const SIZE1: usize = 17 + 17 + 17 + 17;
    const START2: usize = START1 + SIZE1;
    const SIZE2: usize = 17 + 17 + 17 + 17;
    const START3: usize = START2 + SIZE2;
    const SIZE3: usize = 16 + 15 + 11 + 10 + 9 + 8;
    const END3: usize = START3 + SIZE3;
    pub(crate) const NUM_ZONES: usize = 4;
    const ZONES: [core::ops::Range<usize>; NUM_ZONES] =
        [START0..START1, START1..START2, START2..START3, START3..END3];

    fn rgb8_to_power(c: &smart_leds::RGB8) -> u32 {
        let tmp = 122 * c.r as u32 + 121 * c.g as u32 + 121 * c.b as u32;
        tmp / 2550
    }

    fn estimate_current(data: &[smart_leds::RGB8]) -> u32 {
        data.iter().map(|c| rgb8_to_power(c)).sum::<u32>()
    }

    pub fn estimate_current_all(data: &[smart_leds::RGB8; NUM_LEDS]) -> [u32; NUM_ZONES] {
        let mut out = [0; NUM_ZONES];
        for (i, range) in ZONES.iter().cloned().enumerate() {
            out[i] = 78 + estimate_current(&data[range]);
        }
        out
    }

    pub fn limit_current(
        data: &mut [smart_leds::RGB8; NUM_LEDS],
        limit: &[u32; NUM_ZONES],
    ) -> [Option<u32>; NUM_ZONES] {
        // const LIMIT: u32 = 1100;
        let mut ret = [None; NUM_ZONES];
        for (i, range) in ZONES.iter().cloned().enumerate() {
            let data = &mut data[range];
            let current = 78 + estimate_current(data);

            if current <= limit[i] {
                continue;
            }
            const MUL: u32 = 1000;
            let f = limit[i] * MUL / current;
            // let f = LIMIT as f32 / current as f32;

            data.iter_mut().for_each(|v| {
                v.r = ((v.r as u32) * f / MUL) as u8;
                v.g = ((v.g as u32) * f / MUL) as u8;
                v.b = ((v.b as u32) * f / MUL) as u8;
                // v.r = ((v.r as f32) * f) as u8;
                // v.g = ((v.g as f32) * f) as u8;
                // v.b = ((v.b as f32) * f) as u8;
            });
            ret[i] = Some(f);
        }
        ret
    }
}

const CURRENT_MAX: u32 = 1100;
const CURRENT_RATED: u32 = 500;
const NUM_MEASUREMENTS: usize = 60;
pub struct DynamicLimit {
    measurements: [u32; NUM_MEASUREMENTS],
    i: usize,

    acc: u32,
    acc_count: u32,
    pub limit: u32,
}

impl Default for DynamicLimit {
    fn default() -> Self {
        Self {
            measurements: [0; NUM_MEASUREMENTS],
            i: 0,
            limit: CURRENT_MAX,

            acc: 0,
            acc_count: 0,
        }
    }
}

impl DynamicLimit {
    fn commit(&mut self) {
        let current = if self.acc_count != 0 {
            self.acc / self.acc_count
        } else {
            0
        };
        self.acc = 0;
        self.acc_count = 0;

        if self.i >= NUM_MEASUREMENTS {
            self.i %= NUM_MEASUREMENTS;
        }
        self.measurements[self.i] = current;
        self.i += 1;

        let energy = self.measurements.iter().sum::<u32>();
        let budget = NUM_MEASUREMENTS as u32 * CURRENT_RATED;

        if energy > budget {
            let f = energy as f32 / budget as f32;
            let f = if f < 1.0 {
                1.0
            } else if f > 2.0 {
                2.0
            } else {
                f
            };
            self.limit = CURRENT_RATED + ((CURRENT_MAX - CURRENT_RATED) as f32 * (2.0 - f)) as u32;
        }
    }
    fn add_measurement(&mut self, current: u32) {
        self.acc += current;
        self.acc_count += 1;
    }

    fn get_limit(&self) -> u32 {
        self.limit
    }
}

#[rtic::app(device = hal::stm32, peripherals = true, monotonic = rtic::cyccnt::CYCCNT)]
const APP: () = {
    struct Resources {
        timer: Timer<stm32::TIM7>,
        disp: GraphicsMode<
            I2CInterface<
                I2c<
                    I2C1,
                    (
                        PB6<Alternate<hal::gpio::AF4, Output<OpenDrain>>>,
                        PB7<Alternate<hal::gpio::AF4, Output<OpenDrain>>>,
                    ),
                >,
            >,
            DisplaySize128x64,
        >,
        led_strip_dev: ws2812_spi::Ws2812<
            Spi<
                hal::pac::SPI1,
                (
                    PA5<Alternate<hal::gpio::AF5, Input<Floating>>>,
                    PA6<Alternate<hal::gpio::AF5, Input<Floating>>>,
                    PA7<Alternate<hal::gpio::AF5, Input<Floating>>>,
                ),
            >,
        >,
        rainbow: Rainbow,
        led_strip_data: [smart_leds::RGB8; 291],
        led_strip_current: [u32; 4],
        c: u8,
        limit_f: [Option<u32>; 4],
        dynamic_limit: [DynamicLimit; 4],
    }

    #[init(schedule = [refresh_display, refresh_led_strip])]
    fn init(mut cx: init::Context) -> init::LateResources {
        let mut rcc = cx.device.RCC.constrain();
        let mut flash = cx.device.FLASH.constrain();
        let mut pwr = cx.device.PWR.constrain(&mut rcc.apb1r1);
        let mut cp = cx.core;

        // software tasks won't work without this:
        cp.DCB.enable_trace();
        cp.DWT.enable_cycle_counter();

        let clocks = rcc
            .cfgr
            .sysclk(64.mhz())
            .pclk1(16.mhz())
            .pclk2(64.mhz())
            .freeze(&mut flash.acr, &mut pwr);

        // ================================================================================
        // Set up Timer interrupt
        let mut timer = Timer::tim7(cx.device.TIM7, 4.khz(), clocks, &mut rcc.apb1r1);
        timer.listen(Event::TimeOut);

        // ================================================================================
        // set up OLED i2c
        let mut gpiob = cx.device.GPIOB.split(&mut rcc.ahb2);
        let mut scl = gpiob
            .pb6
            .into_open_drain_output(&mut gpiob.moder, &mut gpiob.otyper);
        scl.internal_pull_up(&mut gpiob.pupdr, true);
        let scl = scl.into_af4(&mut gpiob.moder, &mut gpiob.afrl);
        let mut sda = gpiob
            .pb7
            .into_open_drain_output(&mut gpiob.moder, &mut gpiob.otyper);
        sda.internal_pull_up(&mut gpiob.pupdr, true);
        let sda = sda.into_af4(&mut gpiob.moder, &mut gpiob.afrl);

        let mut i2c = I2c::i2c1(
            cx.device.I2C1,
            (scl, sda),
            800.khz(),
            clocks,
            &mut rcc.apb1r1,
        );

        let interface = I2CDIBuilder::new().init(i2c);
        let mut disp: GraphicsMode<_, _> = Builder::new()
            // .with_size(DisplaySize::Display128x64NoOffset)
            .connect(interface)
            .into();
        disp.init().unwrap();
        disp.flush().unwrap();

        disp.write("hello world xxx!", None);
        disp.flush().unwrap();
        cx.schedule
            .refresh_display(cx.start + REFRESH_DISPLAY_PERIOD.cycles())
            .unwrap();

        // ================================================================================
        // setup smart-led strip
        let mut gpioa = cx.device.GPIOA.split(&mut rcc.ahb2);
        let (sck, miso, mosi) = {
            (
                gpioa.pa5.into_af5(&mut gpioa.moder, &mut gpioa.afrl),
                gpioa.pa6.into_af5(&mut gpioa.moder, &mut gpioa.afrl),
                gpioa.pa7.into_af5(&mut gpioa.moder, &mut gpioa.afrl),
            )
        };

        // Configure SPI with 3Mhz rate
        let spi = Spi::spi1(
            cx.device.SPI1,
            (sck, miso, mosi),
            ws2812::MODE,
            3_000_000.hz(),
            clocks,
            &mut rcc.apb2,
        );
        let led_strip_dev = Ws2812::new(spi);

        cx.schedule
            .refresh_led_strip(cx.start + REFRESH_LED_STRIP_PERIOD.cycles())
            .unwrap();

        // Initialization of late resources
        init::LateResources {
            timer,
            disp,
            led_strip_dev,
            rainbow: Rainbow::step_phase(1, 1),
            led_strip_data: [mocca_matrix_rtic::color::BLACK; 291],
            led_strip_current: [0; 4],
            c: 0,
            limit_f: [None; 4],
            dynamic_limit: Default::default(),
        }
    }

    // #[task(binds = TIM7, resources = [timer,  , max, delta, is_on2, led], priority = 3)]
    // fn tim7(cx: tim7::Context) {
    //     cx.resources.timer.clear_interrupt(Event::TimeOut);
    //     // if !*cx.resources.is_on {
    //     //     cx.resources.led.set_high().unwrap();
    //     //     *cx.resources.is_on = true;
    //     // } else {
    //     //     cx.resources.led.set_low().unwrap();
    //     //     *cx.resources.is_on = false;
    //     // }
    //     cx.resources.timer;
    //     while *cx.resources.cur > *cx.resources.max {
    //         // *cx.resources.delta = -1;
    //         *cx.resources.cur -= *cx.resources.max;
    //     }
    //     while *cx.resources.cur < 0 {
    //         // *cx.resources.delta = 1;
    //         *cx.resources.cur += *cx.resources.max;
    //     }
    //     //let duty = GAMMA8[*cx.resources.cur as usize] as i32 * *cx.resources.max / 255;
    //     let duty = *cx.resources.cur;
    //     cx.resources.pwm.set_duty(duty as u32);
    //     // cx.resources.pwm.set_duty(*cx.resources.timer);
    //     // cx.resources.pwm.set_duty(*cx.resources.max);
    //     *cx.resources.cur += *cx.resources.delta;

    //     if *cx.resources.is_on2 {
    //         cx.resources.led.set_low().unwrap();
    //         *cx.resources.is_on2 = false;
    //     } else {
    //         cx.resources.led.set_high().unwrap();
    //         *cx.resources.is_on2 = true;
    //     }
    // }
    // #[task(binds = EXTI15_10, resources = [is_on, button, delta], priority = 2)]
    // fn button(mut cx: button::Context) {
    //     // cx.resources.timer.clear_interrupt(Event::TimeOut);
    //     //cx.resourcescx.resources.button.is_high()
    //     // *cx.resources.is_on = !*cx.resources.is_on;

    //     // if cx.resources.button.is_high().unwrap() {
    //     //     return;
    //     // }
    //     if cx.resources.button.check_interrupt() {
    //         // if we don't clear this bit, the ISR would trigger indefinitely
    //         cx.resources.button.clear_interrupt_pending_bit();
    //     }
    //     let delta = if !*cx.resources.is_on {
    //         // cx.resources.led.set_high().unwrap();
    //         *cx.resources.is_on = true;
    //         1
    //     } else {
    //         // cx.resources.led.set_low().unwrap();
    //         *cx.resources.is_on = false;
    //         -1
    //     };

    //     cx.resources.delta.lock(|x: &mut i32| *x = delta);
    // }

    #[task(schedule=[refresh_display], resources = [disp, led_strip_current, limit_f, dynamic_limit], priority = 1)]
    fn refresh_display(mut cx: refresh_display::Context) {
        let mut text = String::<U32>::new();

        let a = cx.resources.led_strip_current.lock(|x| x.clone());
        let limit_f = cx.resources.limit_f.lock(|x| x.clone());

        for (i, c) in a.iter().enumerate() {
            text.clear();
            let limit_f = match limit_f[i] {
                Some(l) => l as i32,
                None => -1,
            };
            let limit_dyn = cx.resources.dynamic_limit.lock(|x| x[i].get_limit());

            write!(&mut text, "I({}): {} {}", i, c, limit_dyn).unwrap();
            cx.resources.disp.write(&text, Some(1 + i as i32));
        }
        text.clear();
        write!(&mut text, "{:?}", cx.scheduled).unwrap();
        cx.resources.disp.write(&text, Some(5));
        cx.resources.disp.flush().unwrap();
        cx.schedule
            .refresh_display(cx.scheduled + REFRESH_DISPLAY_PERIOD.cycles())
            .unwrap();
    }
    #[task(schedule=[refresh_led_strip], resources = [led_strip_dev, rainbow, led_strip_data, led_strip_current, c, limit_f, dynamic_limit], priority = 3)]
    fn refresh_led_strip(mut cx: refresh_led_strip::Context) {
        // let mut rainbow = brightness(cx.resources.rainbow, 64);

        let mut c = cx.resources.rainbow.next().unwrap();
        let mut rainbow = Rainbow::step_phase(1, *cx.resources.c);
        for i in 0..291 {
            // cx.resources.led_strip_data[i] = c; // RGB8::new(255, 0, 255);

            if i % 4 == 3 {
                c = rainbow.next().unwrap();
                c.b = 0;
            }

            cx.resources.led_strip_data[i] = c;
            // cx.resources.led_strip_data[i] =
            //     RGB8::new(*cx.resources.c, *cx.resources.c, *cx.resources.c);
        }
        *cx.resources.c += 1;

        *cx.resources.led_strip_current =
            power_zones::estimate_current_all(cx.resources.led_strip_data);

        let mut limit = [0u32; power_zones::NUM_ZONES];
        for i in 0..power_zones::NUM_ZONES {
            cx.resources.dynamic_limit[i].add_measurement(cx.resources.led_strip_current[i]);
            if *cx.resources.c % 32 == 0 {
                cx.resources.dynamic_limit[i].commit();
            }
            limit[i] = cx.resources.dynamic_limit[i].get_limit();
        }

        *cx.resources.limit_f =
            power_zones::limit_current(&mut cx.resources.led_strip_data, &limit);

        cx.resources
            .led_strip_dev
            .write(cx.resources.led_strip_data.iter().cloned())
            .unwrap();

        cx.schedule
            .refresh_led_strip(cx.scheduled + REFRESH_LED_STRIP_PERIOD.cycles())
            .unwrap();
    }

    extern "C" {
        fn COMP();
        fn SDMMC1();
    }
};
