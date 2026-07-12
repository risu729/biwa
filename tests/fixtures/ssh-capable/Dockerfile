FROM lscr.io/linuxserver/openssh-server@sha256:204b64775f7dfb17761ed0644df348ba3cd811628334cc723aa69d61f4db893d

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
