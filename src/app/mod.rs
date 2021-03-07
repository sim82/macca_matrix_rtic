pub mod drawing;
pub mod hexlife;
use crate::prelude::*;

pub trait App {
    fn new() -> Self;
    fn tick(&mut self, led_data: &mut [RGB8; NUM_LEDS]);
}
