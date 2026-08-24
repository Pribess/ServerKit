/// Runs a ServerKit router with a listener supported by this adapter.
pub trait Run<L> {
    /// The value returned by the selected driver.
    type Output;

    /// Serves the router using `listener`.
    fn run(self, listener: L) -> Self::Output;
}
