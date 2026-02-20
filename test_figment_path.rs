use figment::{Figment, Jail, value::magic::RelativePathBuf};

fn main() {
    Jail::expect_with(|jail| {
        jail.create_file("test.toml", "path = \"~/.ssh/key\"\n")?;
        let figment = Figment::from(figment::providers::Toml::file("test.toml"));
        #[derive(serde::Deserialize, Debug)]
        struct Config { path: RelativePathBuf }
        let c: Config = figment.extract()?;
        println!("Resolved path: {:?}", c.path.relative());
        Ok(())
    });
}
