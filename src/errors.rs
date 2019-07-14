use error_chain::error_chain;

error_chain! {
    foreign_links {
        IO(std::io::Error);
        Docopt(docopt::Error);
    }
}
