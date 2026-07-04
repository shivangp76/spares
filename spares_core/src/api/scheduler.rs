use crate::Error;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::review::Rating;

pub fn get_scheduler_ratings(scheduler_name: &str) -> Result<Vec<Rating>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    Ok(scheduler.get_ratings())
}

pub fn resolve_rating_from_score(scheduler_name: &str, score: f64) -> Result<Rating, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    scheduler.rating_from_score(score)
}
