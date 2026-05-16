FROM lscr.io/linuxserver/openssh-server@sha256:29d4e3f887596c4c2fc609f4e07040b08890a238178da400ffa2a602b55245bc

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
