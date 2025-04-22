use crate::{Error, schedulers::get_scheduler_from_string, schema::review::Rating};

pub fn get_scheduler_ratings(scheduler_name: &str) -> Result<Vec<Rating>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    Ok(scheduler.get_ratings())
}
