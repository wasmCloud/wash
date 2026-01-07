wit_bindgen::generate!({ generate_all });

use crate::exports::example::create::create;
struct Component;

impl create::Guest for Component {
    fn create() -> Result<String, String> {
        Ok("CALLING CREATE V2 - COMPONENT UPDATED".to_string())
    }
}
export!(Component);
