use crate::Router;

pub trait Listener {
    type Output;

    fn serve(self, router: Router) -> Self::Output;
}
