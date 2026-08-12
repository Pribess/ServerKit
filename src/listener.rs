use crate::App;

pub trait Listener {
    type Output;

    fn serve(self, application: App) -> Self::Output;
}
