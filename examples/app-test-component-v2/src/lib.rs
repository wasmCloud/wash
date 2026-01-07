wit_bindgen::generate!({ generate_all });

use crate::example::create::create::create;
use crate::example::update::update::update;
use crate::exports::example::action::action;
struct Component;

impl action::Guest for Component {
    fn run() -> Result<String, String> {
        // Call both create() and update() and return both results
        // This allows us to verify both components work after updates
        let create_result = create()?;
        let update_result = update()?;

        Ok(format!(
            "Action result: {} | Update result: {}",
            create_result, update_result
        ))
    }
}
export!(Component);
