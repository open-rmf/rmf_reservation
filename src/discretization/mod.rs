use crate::ReservationRequest;

pub mod fixed_timestep;

pub mod minimal_samples;

pub trait DescretizationStrategy {
    fn discretize(
        &mut self,
        requests: &Vec<Vec<ReservationRequest>>,
    ) -> Vec<Vec<ReservationRequest>>;

    fn remap(&self, ticket_id: &(usize, usize)) -> (usize, usize);
}
