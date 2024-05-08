use crate::ReservationRequestAlternative;

pub mod fixed_timestep;

/// An abstract trait that implements ways to discretize objects
/// Allows for implementation of custom discretization strategy.
pub trait DescretizationStrategy {
    /// Discretize the objects and produce an equivalent
    fn discretize(
        &mut self,
        requests: &Vec<Vec<ReservationRequestAlternative>>,
    ) -> Vec<Vec<ReservationRequestAlternative>>;

    /// Remap the new problem to the old problem.
    fn remap(&self, ticket_id: &(usize, usize)) -> Option<(usize, usize)>;
}
