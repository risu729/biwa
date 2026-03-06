pub fn biwa_cmd(args: &[&str]) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123")
}
