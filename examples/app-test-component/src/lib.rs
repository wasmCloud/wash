wit_bindgen::generate!({ generate_all });

use crate::example::create::create::create;
use crate::example::update::update::update;
use crate::exports::example::action::action;
struct Component;

impl action::Guest for Component {
    fn run() -> Result<String, String> {
        let _ = create();
        let _ = update();

        Ok("Ran action".to_string())
    }
}
export!(Component);
