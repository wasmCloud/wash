wit_bindgen::generate!({ generate_all });

use crate::exports::example::update::update;
struct Component;

impl update::Guest for Component {
    fn update() -> Result<String, String> {
        Ok("CALLING UPDATE".to_string())
    }
}
export!(Component);
