use tokio::runtime::Runtime;

pub struct Error;

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error {}
    }
}
