use smithay::{
    output::Output,
    utils::{Logical, Rectangle},
};

pub trait OutputExt {
    fn geometry(&self) -> Option<Rectangle<i32, Logical>>;
}

impl OutputExt for Output {
    fn geometry(&self) -> Option<Rectangle<i32, Logical>> {
        self.current_mode().map(|mode| {
            let size = self
                .current_transform()
                .transform_size(mode.size)
                .to_f64()
                .to_logical(self.current_scale().fractional_scale())
                .to_i32_round();
            Rectangle::new(self.current_location(), size)
        })
    }
}
