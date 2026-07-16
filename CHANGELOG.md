# Changelog

## [Unreleased]

## [0.1.1](https://github.com/risu729/biwa/compare/v0.1.0...v0.1.1)

### ⛰️ Features


- *(clean)* Add biwa clean command to remove stale remote directories ([#415](https://github.com/risu729/biwa/pull/415)) - ([1070b62](https://github.com/risu729/biwa/commit/1070b624211e73190705389304cb967690ac624b))
- *(config)* Deserialize umask in config as octal string ([#331](https://github.com/risu729/biwa/pull/331)) - ([04b41a7](https://github.com/risu729/biwa/commit/04b41a7d7b90c6573ae0254d78185c0ca6ee0a97))
- *(env)* Support transferring and setting remote env vars ([#339](https://github.com/risu729/biwa/pull/339)) - ([486951a](https://github.com/risu729/biwa/commit/486951ac7a759611f7db7c0bd8e43c7ba2f24be3))
- *(ssh)* Forward Ctrl+C to remote command ([#607](https://github.com/risu729/biwa/pull/607)) - ([958ecde](https://github.com/risu729/biwa/commit/958ecde5245328558b14f6de23cd89d0176f54d0))
- Incorporate hostname into unique project name calculation to prevent cross-machine collisions. ([#265](https://github.com/risu729/biwa/pull/265)) - ([3dcfad5](https://github.com/risu729/biwa/commit/3dcfad54b204cfa9efe114fb880ee88f8affd4cb))
- Implement SSH file synchronization ([#257](https://github.com/risu729/biwa/pull/257)) - ([260f05a](https://github.com/risu729/biwa/commit/260f05a2236573e2ecf40474d0564904ec85a71e))
- Native SSH command execution with async-ssh2-tokio ([#250](https://github.com/risu729/biwa/pull/250)) - ([dab84f9](https://github.com/risu729/biwa/commit/dab84f95d174ef7fb80af7e754a162a0376dabfe))
- Add autocompletion feature and cli reference ([#244](https://github.com/risu729/biwa/pull/244)) - ([95e0375](https://github.com/risu729/biwa/commit/95e0375878bde64223b7c78161338f2d14bfe982))
- Comprehensive Configuration Resolution ([#237](https://github.com/risu729/biwa/pull/237)) - ([82ef9d8](https://github.com/risu729/biwa/commit/82ef9d87e978ab0c97161587c25dbc123acb8b84))

### 🐛 Bug Fixes


- *(cli)* Buffer configuration load logs ([#390](https://github.com/risu729/biwa/pull/390)) - ([2035e35](https://github.com/risu729/biwa/commit/2035e35b7051fcc06cbd8e43fb532e9eacc41255))
- *(clippy)* Enable rust 1.95 lints ([#596](https://github.com/risu729/biwa/pull/596)) - ([6917b55](https://github.com/risu729/biwa/commit/6917b556e8c05b36e263a1b1cd11b7eff7863e8e))
- *(config)* Error on missing required ssh settings ([#397](https://github.com/risu729/biwa/pull/397)) - ([a9f04d6](https://github.com/risu729/biwa/commit/a9f04d6baaec4576cab2cc35a75c5140f7bbdc75))
- *(deps)* Update rust crate tokio to v1.52.4 ([#867](https://github.com/risu729/biwa/pull/867)) - ([094375e](https://github.com/risu729/biwa/commit/094375e617c77a49fac85d37f81c412223188b5c))
- *(deps)* Update rust crate clap to v4.6.2 ([#863](https://github.com/risu729/biwa/pull/863)) - ([54c31fd](https://github.com/risu729/biwa/commit/54c31fd0088cd4bd510e973ed2ad6e1a50cd563e))
- *(deps)* Update rust crate ignore to v0.4.29 ([#860](https://github.com/risu729/biwa/pull/860)) - ([717be85](https://github.com/risu729/biwa/commit/717be85e4fe1354b53997d4ef6e599ef0347cf33))
- *(deps)* Update rust crate globset to v0.4.19 ([#859](https://github.com/risu729/biwa/pull/859)) - ([6b0a778](https://github.com/risu729/biwa/commit/6b0a7781662a9fb26f528b2daefbc3aecd33e3eb))
- *(deps)* Update rust crate toml to v1.1.3 ([#856](https://github.com/risu729/biwa/pull/856)) - ([4c5a640](https://github.com/risu729/biwa/commit/4c5a640e443513379f3bed1bf3b9c9042d42d1f9))
- *(deps)* Update rust crate usage-lib to v3.5.5 ([#853](https://github.com/risu729/biwa/pull/853)) - ([8bb9932](https://github.com/risu729/biwa/commit/8bb9932959ff54487d815dfc77191d9ccf0baf27))
- *(deps)* Update rust crate russh to v0.62.2 ([#844](https://github.com/risu729/biwa/pull/844)) - ([621b353](https://github.com/risu729/biwa/commit/621b353e64cf10c3472f6874709b70308a318531))
- *(deps)* Update rust crate russh to v0.61.1 [security] ([#832](https://github.com/risu729/biwa/pull/832)) - ([82a840b](https://github.com/risu729/biwa/commit/82a840b0b3dd007733538282c4deb8836c3b91c8))
- *(deps)* Update rust crate indicatif to v0.18.6 ([#837](https://github.com/risu729/biwa/pull/837)) - ([6f955d1](https://github.com/risu729/biwa/commit/6f955d1866e616471c7b668447bb497e910ce9c3))
- *(deps)* Update rust crate usage-lib to v3.5.4 ([#838](https://github.com/risu729/biwa/pull/838)) - ([2fb82a0](https://github.com/risu729/biwa/commit/2fb82a0ba1e1cf273c4dcbc8bc89c1f90de6f4a4))
- *(deps)* Update rust crate console to v0.16.4 ([#835](https://github.com/risu729/biwa/pull/835)) - ([97a2797](https://github.com/risu729/biwa/commit/97a27978bcce9133d89c80e1bec4ce62c411054a))
- *(deps)* Update rust crate humantime to v2.4.0 ([#839](https://github.com/risu729/biwa/pull/839)) - ([c0d6aff](https://github.com/risu729/biwa/commit/c0d6aff57324df573b8872411ae60a2462f2d812))
- *(deps)* Update rust crate bytes to v1.12.1 ([#834](https://github.com/risu729/biwa/pull/834)) - ([98f4dea](https://github.com/risu729/biwa/commit/98f4deaa3c84e68acc328790017ca73afe983d9b))
- *(deps)* Update rust crate ignore to v0.4.28 ([#836](https://github.com/risu729/biwa/pull/836)) - ([1ba7133](https://github.com/risu729/biwa/commit/1ba7133bcf13549bcce92ef221a069c32ad7262f))
- *(deps)* Restore Renovate Cargo extraction ([#831](https://github.com/risu729/biwa/pull/831)) - ([0e7c26e](https://github.com/risu729/biwa/commit/0e7c26ece0fed5d507fd7b7618b5491a2a4f18d2))
- *(deps)* Update rust crate usage-lib to v3.5.3 ([#770](https://github.com/risu729/biwa/pull/770)) - ([c2343aa](https://github.com/risu729/biwa/commit/c2343aa1080010bf27798518f0ac29e16570c909))
- *(deps)* Update rust crate ignore to v0.4.26 ([#718](https://github.com/risu729/biwa/pull/718)) - ([c9d7414](https://github.com/risu729/biwa/commit/c9d74149e992d03d2eb2a90a1bd404179f381bcb))
- *(deps)* Update rust crate serde_json to v1.0.150 ([#670](https://github.com/risu729/biwa/pull/670)) - ([c57eaf4](https://github.com/risu729/biwa/commit/c57eaf45acded92c52cc2884015cf88e436b43f6))
- *(deps)* Update rust crate usage-lib to v3.4.0 ([#705](https://github.com/risu729/biwa/pull/705)) - ([2a58959](https://github.com/risu729/biwa/commit/2a5895931ccef36c748804941e0e03015532e2e5))
- *(deps)* Update rust crate russh-sftp to v2.2.2 ([#672](https://github.com/risu729/biwa/pull/672)) - ([26f6b14](https://github.com/risu729/biwa/commit/26f6b14b8ecc46a97e0729cde5a280f769346686))
- *(deps)* Update rust crate chrono to v0.4.45 ([#717](https://github.com/risu729/biwa/pull/717)) - ([5bd44ad](https://github.com/risu729/biwa/commit/5bd44ad8cfeae58ce45454e16367e5268feedb8a))
- *(deps)* Update rust crate itertools to v0.15.0 ([#744](https://github.com/risu729/biwa/pull/744)) - ([ecd401e](https://github.com/risu729/biwa/commit/ecd401e413a88f18db719e0522844e0afca02944))
- *(deps)* Update rust crate bytes to v1.12.0 ([#748](https://github.com/risu729/biwa/pull/748)) - ([de2c55e](https://github.com/risu729/biwa/commit/de2c55ec697d4f36180cb754341abf0351f8c485))
- *(deps)* Update rust crate russh to v0.60.3 ([#629](https://github.com/risu729/biwa/pull/629)) - ([7067eb6](https://github.com/risu729/biwa/commit/7067eb69deb9466976b5cee736a6d819125cc41e))
- *(deps)* Update rust crate nix to v0.31.3 ([#622](https://github.com/risu729/biwa/pull/622)) - ([e336021](https://github.com/risu729/biwa/commit/e33602199ac44e7e827f2506f643b212194412d0))
- *(deps)* Update rust crate russh-sftp to v2.1.2 ([#588](https://github.com/risu729/biwa/pull/588)) - ([b4e99fb](https://github.com/risu729/biwa/commit/b4e99fb1fee63f22d7d0efdad5c2f611ff36157c))
- *(deps)* Update rust crate russh to v0.60.2 ([#599](https://github.com/risu729/biwa/pull/599)) - ([646ec55](https://github.com/risu729/biwa/commit/646ec550a755c6cd2bfe64c326b72c1f1dea2a63))
- *(deps)* Update rust crate russh to v0.60.1 [security] ([#580](https://github.com/risu729/biwa/pull/580)) - ([c93c0c8](https://github.com/risu729/biwa/commit/c93c0c84eea900e96af8157426222e7f2447e82a))
- *(deps)* Update rust crate usage-lib to v3.3.0 ([#570](https://github.com/risu729/biwa/pull/570)) - ([7ca25cb](https://github.com/risu729/biwa/commit/7ca25cb3a74e36b1439ecf23f2db586c66e37073))
- *(deps)* Update rust crate tokio to v1.52.3 ([#591](https://github.com/risu729/biwa/pull/591)) - ([31be419](https://github.com/risu729/biwa/commit/31be4196471d39dd0f93ef1c2486303109f274dc))
- *(deps)* Update rust crate tokio to v1.52.1 ([#553](https://github.com/risu729/biwa/pull/553)) - ([a665d47](https://github.com/risu729/biwa/commit/a665d475f85cf2b9bf7dbeae3c228edc35046b35))
- *(deps)* Update rust crate clap to v4.6.1 ([#544](https://github.com/risu729/biwa/pull/544)) - ([86e5e3e](https://github.com/risu729/biwa/commit/86e5e3ea2c0492285356f81bd1fd82307b5073c0))
- *(deps)* Update rust crate tokio to v1.52.0 ([#539](https://github.com/risu729/biwa/pull/539)) - ([bb2fdab](https://github.com/risu729/biwa/commit/bb2fdabc60a4c071e4259be64b2d0a0874eb62a4))
- *(deps)* Update rust crate tokio to v1.51.1 ([#501](https://github.com/risu729/biwa/pull/501)) - ([9f70f3e](https://github.com/risu729/biwa/commit/9f70f3e1fb7c01e6e03cbc77b0216d5319cf10c1))
- *(deps)* Update rust crate russh to v0.60.0 ([#490](https://github.com/risu729/biwa/pull/490)) - ([9ea45c0](https://github.com/risu729/biwa/commit/9ea45c0ba98ee75c147dfc3160bf9c1174de2e65))
- *(deps)* Update rust crate tokio to v1.51.0 ([#486](https://github.com/risu729/biwa/pull/486)) - ([ee8f266](https://github.com/risu729/biwa/commit/ee8f26668885007b8b7b5036020d797c009d80db))
- *(deps)* Update rust crate russh to v0.59.0 ([#448](https://github.com/risu729/biwa/pull/448)) - ([839ab2c](https://github.com/risu729/biwa/commit/839ab2c2a062da2ff88127bed6729a0bf8f989f8))
- *(deps)* Update rust crate sha2 to v0.11.0 ([#438](https://github.com/risu729/biwa/pull/438)) - ([f28fbc6](https://github.com/risu729/biwa/commit/f28fbc6afbb1509bdb4e7ee9e526db5b154f615b))
- *(deps)* Update rust crate toml to v1.1.2 ([#474](https://github.com/risu729/biwa/pull/474)) - ([e70229a](https://github.com/risu729/biwa/commit/e70229a6fb1e1fffe5599966167e2f0c967ee3fb))
- *(deps)* Update rust crate toml to v1.1.1 ([#470](https://github.com/risu729/biwa/pull/470)) - ([be54b4a](https://github.com/risu729/biwa/commit/be54b4ac452f6f8fe81d056ca6f073129f3fe138))
- *(deps)* Update rust crate usage-lib to v3.2.0 ([#430](https://github.com/risu729/biwa/pull/430)) - ([6146894](https://github.com/risu729/biwa/commit/6146894502e522db3ab7d8615d169384825fe6fb))
- *(deps)* Update rust crate toml to v1.1.0 ([#427](https://github.com/risu729/biwa/pull/427)) - ([1edf1f0](https://github.com/risu729/biwa/commit/1edf1f0c433c9dfdbdd0e2325f99cb0264cf6c67))
- *(deps)* Update rust crate usage-lib to v3.1.0 ([#419](https://github.com/risu729/biwa/pull/419)) - ([721aefd](https://github.com/risu729/biwa/commit/721aefdc1b7b4ec3765994f66872915a1584ebd1))
- *(deps)* Update rust crate russh to v0.58.0 ([#284](https://github.com/risu729/biwa/pull/284)) - ([fa6c7bd](https://github.com/risu729/biwa/commit/fa6c7bd67302203f0ab3f9f56e7b67a53c3abc6c))
- *(deps)* Update rust crate toml to v1.0.7 ([#386](https://github.com/risu729/biwa/pull/386)) - ([07fc7df](https://github.com/risu729/biwa/commit/07fc7dff1634d30131d58003ceae4afd9d9ec880))
- *(deps)* Update rust crate usage-lib to v3 ([#348](https://github.com/risu729/biwa/pull/348)) - ([6322967](https://github.com/risu729/biwa/commit/6322967b275c094aec558bf578ba1ffee7c6b20f))
- *(deps)* Update rust crate console to v0.16.3 ([#344](https://github.com/risu729/biwa/pull/344)) - ([63313f2](https://github.com/risu729/biwa/commit/63313f2aa864dff851c8738cf8a88632960c4dc7))
- *(deps)* Update rust crate tracing-subscriber to v0.3.23 ([#345](https://github.com/risu729/biwa/pull/345)) - ([1c84181](https://github.com/risu729/biwa/commit/1c841811db37a3a763f6ebeb27630bc0889fe74d))
- *(deps)* Update rust crate clap to v4.6.0 ([#340](https://github.com/risu729/biwa/pull/340)) - ([c37af8d](https://github.com/risu729/biwa/commit/c37af8d8423f19fe323367ee039a9e76057692a0))
- *(deps)* Update rust crate clap to v4.5.61 ([#336](https://github.com/risu729/biwa/pull/336)) - ([56e2767](https://github.com/risu729/biwa/commit/56e276734b5da2f85bcb8da2ce8b23437768f41b))
- *(deps)* Update rust crate toml to v1.0.6 ([#320](https://github.com/risu729/biwa/pull/320)) - ([36c06a2](https://github.com/risu729/biwa/commit/36c06a24526ff334dc09142eacb849784cece7b8))
- *(deps)* Pin dependencies ([#267](https://github.com/risu729/biwa/pull/267)) - ([a0c6fb2](https://github.com/risu729/biwa/commit/a0c6fb2763666358a4731d2a7b97beb7aa1ac67b))
- *(deps)* Upgrade crates ([#251](https://github.com/risu729/biwa/pull/251)) - ([bc2b381](https://github.com/risu729/biwa/commit/bc2b381ac0cdb4a6fc3f4dbb4f70425f317f06fe))
- *(deps)* Update rust crate clap to v4.5.59 ([#224](https://github.com/risu729/biwa/pull/224)) - ([3df51bf](https://github.com/risu729/biwa/commit/3df51bf28f9c1741fc412ee94f09756451a61734))
- *(deps)* Update rust crate tokio to v1.49.0 ([#54](https://github.com/risu729/biwa/pull/54)) - ([fef8b31](https://github.com/risu729/biwa/commit/fef8b310025aa14d7807753b7a5f32ca5bcbff07))
- *(deps)* Update rust crate clap to v4.5.58 ([#197](https://github.com/risu729/biwa/pull/197)) - ([7ab0f0d](https://github.com/risu729/biwa/commit/7ab0f0d25f967ac4d234510c2a97418b603b271e))
- *(deps)* Update rust crate clap to v4.5.56 ([#188](https://github.com/risu729/biwa/pull/188)) - ([8ed3650](https://github.com/risu729/biwa/commit/8ed3650240858ab033d5b5e6017445cc739dc5ca))
- *(deps)* Update rust crate clap to v4.5.55 ([#184](https://github.com/risu729/biwa/pull/184)) - ([343ba3f](https://github.com/risu729/biwa/commit/343ba3fa7649307ff9d5a698f83a8c9bf48e4db0))
- *(deps)* Update rust crate clap to v4.5.54 ([#155](https://github.com/risu729/biwa/pull/155)) - ([2931ce8](https://github.com/risu729/biwa/commit/2931ce8125dd10e2c07cf78bfa1eab58074f6af9))
- *(deps)* Update rust crate tracing to v0.1.44 ([#141](https://github.com/risu729/biwa/pull/141)) - ([1e806f4](https://github.com/risu729/biwa/commit/1e806f4d996254de25a296daa27902c1bdf35311))
- *(deps)* Update tokio-tracing monorepo ([#113](https://github.com/risu729/biwa/pull/113)) - ([7459a5a](https://github.com/risu729/biwa/commit/7459a5a4f914b4bac51674a31084bac1ddd75e62))
- *(deps)* Update rust crate tracing-subscriber to v0.3.21 ([#108](https://github.com/risu729/biwa/pull/108)) - ([9bd2408](https://github.com/risu729/biwa/commit/9bd2408d95378cb9e635e7ae0c2e11f018e97791))
- *(deps)* Update rust crate clap to v4.5.53 ([#95](https://github.com/risu729/biwa/pull/95)) - ([4cc8d9f](https://github.com/risu729/biwa/commit/4cc8d9f70ad0a2306406fec589e6f85cb16a02a3))
- *(deps)* Update rust crate clap to v4.5.52 ([#93](https://github.com/risu729/biwa/pull/93)) - ([e26701c](https://github.com/risu729/biwa/commit/e26701c917d7cb9ee71aa6a9c8967df0b9c48df1))
- *(deps)* Update rust crate clap to v4.5.51 ([#71](https://github.com/risu729/biwa/pull/71)) - ([33e5d85](https://github.com/risu729/biwa/commit/33e5d85a67cee6b57306437bc06f3bf59ca3731e))
- *(deps)* Update rust crate clap to v4.5.50 ([#62](https://github.com/risu729/biwa/pull/62)) - ([5ca933e](https://github.com/risu729/biwa/commit/5ca933e320a864a0b02332182f9b994d7fb9099a))
- *(deps)* Update rust crate clap to v4.5.49 ([#52](https://github.com/risu729/biwa/pull/52)) - ([e811cc3](https://github.com/risu729/biwa/commit/e811cc3fd9b23712acedd243ed308b8bca56d8b2))
- *(deps)* Update rust crate clap to v4.5.48 ([#10](https://github.com/risu729/biwa/pull/10)) - ([364fe1a](https://github.com/risu729/biwa/commit/364fe1aea35052d5deed2af10c7dd1e859a5797d))
- *(mise)* Migrate docs setup to deps ([#592](https://github.com/risu729/biwa/pull/592)) - ([63a18e1](https://github.com/risu729/biwa/commit/63a18e1ba5ea9f27d7500f99f0395380ae0ce103))
- *(run)* Forward stdin to remote commands ([#400](https://github.com/risu729/biwa/pull/400)) - ([b89e9e7](https://github.com/risu729/biwa/commit/b89e9e7a80f6b9e8581b8af865195413209b9c18))
- *(schema)* Mark fields with defaults as optional in JSON schema ([#332](https://github.com/risu729/biwa/pull/332)) - ([5409e96](https://github.com/risu729/biwa/commit/5409e96656561e48bb9e727b9c83d089de2d2ae0))
- *(ssh)* Drain silent command output ([#375](https://github.com/risu729/biwa/pull/375)) - ([474d798](https://github.com/risu729/biwa/commit/474d798d22d2f9de74db91323cc9e22d2d8c271a))
- *(sync)* Synchronize empty directories ([#378](https://github.com/risu729/biwa/pull/378)) - ([93a28e7](https://github.com/risu729/biwa/commit/93a28e7bb947dd3842900bf50be9c9aedd88de69))
- *(sync)* Check file limit before SSH connect ([#377](https://github.com/risu729/biwa/pull/377)) - ([453dfe9](https://github.com/risu729/biwa/commit/453dfe9b9c25cbde16bf9dfbe3448530c33c3633))
- Use git root as default sync root ([#605](https://github.com/risu729/biwa/pull/605)) - ([14829fc](https://github.com/risu729/biwa/commit/14829fc419214e84d36785e1b7fa3336bbede9c0))
- Unify behavior of implicit biwa execution and biwa run ([#329](https://github.com/risu729/biwa/pull/329)) - ([dae941d](https://github.com/risu729/biwa/commit/dae941de9258ee79621ab84b497f24437dc0da21))
- Hide internal traces from end-user errors ([#327](https://github.com/risu729/biwa/pull/327)) - ([a9450ba](https://github.com/risu729/biwa/commit/a9450ba8be61e1427dc02676a569179bc3d6beab))

### ⚡ Performance


- *(ssh)* Reuse single SSH connection for sync and run ([#412](https://github.com/risu729/biwa/pull/412)) - ([d1da911](https://github.com/risu729/biwa/commit/d1da9117a15628e9b05077d88b785f81a886cc8d))

### 🚜 Refactor


- *(cli)* Remove log config and deferred loading ([#398](https://github.com/risu729/biwa/pull/398)) - ([7c5dd95](https://github.com/risu729/biwa/commit/7c5dd952679cd58ca59c3ac0e0d579e9fe7db0c3))
- *(config)* Move check_remote_root to config validation phase ([#379](https://github.com/risu729/biwa/pull/379)) - ([95192fb](https://github.com/risu729/biwa/commit/95192fbfb72215985aa47716b9f0c8bfe4a7fa4a))
- *(config)* Migrate from figment to confique ([#247](https://github.com/risu729/biwa/pull/247)) - ([eaa3693](https://github.com/risu729/biwa/commit/eaa369384ff6a4e0652a5a75f68cb494ae9444b0))
- *(ssh)* Replace async-ssh2-tokio with custom russh client wrapper ([#407](https://github.com/risu729/biwa/pull/407)) - ([6697920](https://github.com/risu729/biwa/commit/669792080998b8777b514270044e1e0e6a6c31ee))
- *(sync)* Extract path helpers ([#595](https://github.com/risu729/biwa/pull/595)) - ([8e28434](https://github.com/risu729/biwa/commit/8e28434102bacf43d0a2f1fb6926e448c5924a0a))
- Migrate to color-eyre for improved error reporting ([#258](https://github.com/risu729/biwa/pull/258)) - ([ca16e0b](https://github.com/risu729/biwa/commit/ca16e0b84fc77cf56c1051b2760d90b7b2f919f0))

### 📚 Documentation


- Refresh logo and README ([#846](https://github.com/risu729/biwa/pull/846)) - ([10cf11e](https://github.com/risu729/biwa/commit/10cf11e8aa965336a86584e6c50115727f526e9d))
- Initialize docs with VitePress and Cloudflare Workers ([#213](https://github.com/risu729/biwa/pull/213)) - ([2ba4ebc](https://github.com/risu729/biwa/commit/2ba4ebc052e24855625104bd6ba1d54a9014186c))

### 🧪 Testing


- *(auth)* Remove unsafe env mutation in try_agent tests ([#382](https://github.com/risu729/biwa/pull/382)) - ([4f27ded](https://github.com/risu729/biwa/commit/4f27ded19131f3f7ca7d553219034c5e22785755))
- Use pretty_assertions ([#260](https://github.com/risu729/biwa/pull/260)) - ([e165a3d](https://github.com/risu729/biwa/commit/e165a3d9f103dbf82d190255fb8cc1fc1e0e0085))

### 🎨 Styling


- *(cargo)* Sort Cargo.toml ([#252](https://github.com/risu729/biwa/pull/252)) - ([769c839](https://github.com/risu729/biwa/commit/769c839c02a7237ebde2a644487b431c78b07bbf))

### 🧹 Chore


- *(clippy)* Enable nursery/restiction rules ([#253](https://github.com/risu729/biwa/pull/253)) - ([61545e7](https://github.com/risu729/biwa/commit/61545e77d68cbeb09679380707d5602b730a75a0))
- *(deps)* Update rust crate ctor to v1.0.9 ([#864](https://github.com/risu729/biwa/pull/864)) - ([f39fd37](https://github.com/risu729/biwa/commit/f39fd372a457c724fabf344350f8380b61654245))
- *(deps)* Lock file maintenance ([#850](https://github.com/risu729/biwa/pull/850)) - ([9054cc8](https://github.com/risu729/biwa/commit/9054cc8a76ce62ce1b0da4739c3126b6cb79d014))
- *(deps)* Update dependency rust to v1.97.0 ([#821](https://github.com/risu729/biwa/pull/821)) - ([4b0299e](https://github.com/risu729/biwa/commit/4b0299efa2ec86ae0c1d964f994a42338c7a001c))
- *(deps)* Update rust crate ctor to v1.0.8 ([#833](https://github.com/risu729/biwa/pull/833)) - ([acff176](https://github.com/risu729/biwa/commit/acff176684da2338c74af1083cc638246a6b66de))
- *(deps)* Lock file maintenance ([#830](https://github.com/risu729/biwa/pull/830)) - ([8ae43e6](https://github.com/risu729/biwa/commit/8ae43e6cfe7d589733a52a06a9c047fdfaf989aa))
- *(deps)* Update dependency tombi to v0.11.6 ([#669](https://github.com/risu729/biwa/pull/669)) - ([a16e715](https://github.com/risu729/biwa/commit/a16e715319087396c82cda9dde6597aa3ddf1927))
- *(deps)* Lock file maintenance ([#765](https://github.com/risu729/biwa/pull/765)) - ([7c1b5d0](https://github.com/risu729/biwa/commit/7c1b5d09de0a65c1cbdc5eb3880c682c379157fe))
- *(deps)* Update rust crate insta to v1.48.0 ([#738](https://github.com/risu729/biwa/pull/738)) - ([37cc1c2](https://github.com/risu729/biwa/commit/37cc1c2064a5d36dd833c8524f697d2cacc6a273))
- *(deps)* Update rust crate serial_test to v3.5.0 ([#703](https://github.com/risu729/biwa/pull/703)) - ([6c8a343](https://github.com/risu729/biwa/commit/6c8a343b07b8e6d397f6984c7adddae93fe71f8a))
- *(deps)* Lock file maintenance ([#728](https://github.com/risu729/biwa/pull/728)) - ([2de7583](https://github.com/risu729/biwa/commit/2de75838735ed2a6510be65e3e48b95692905791))
- *(deps)* Update rust crate ctor to v1.0.7 ([#698](https://github.com/risu729/biwa/pull/698)) - ([03972b8](https://github.com/risu729/biwa/commit/03972b8d7916bb71d7a0ca55e4bcf5fe50517904))
- *(deps)* Lock file maintenance ([#707](https://github.com/risu729/biwa/pull/707)) - ([c699f30](https://github.com/risu729/biwa/commit/c699f30c0136e6728daee8a297c7cd1bdb3c4aa2))
- *(deps)* Lock file maintenance ([#646](https://github.com/risu729/biwa/pull/646)) - ([3a24b31](https://github.com/risu729/biwa/commit/3a24b31b0fc49490bb4855a4b06558b7eb8d3ffa))
- *(deps)* Update rust crate ctor to v1.0.6 ([#639](https://github.com/risu729/biwa/pull/639)) - ([7089936](https://github.com/risu729/biwa/commit/708993607862c4c164b75f1a78f83a93921554ef))
- *(deps)* Update rust crate ctor to v1.0.5 ([#615](https://github.com/risu729/biwa/pull/615)) - ([7355620](https://github.com/risu729/biwa/commit/7355620dd08ff0870edcb1621ed362f80603874b))
- *(deps)* Lock file maintenance ([#613](https://github.com/risu729/biwa/pull/613)) - ([54b5bee](https://github.com/risu729/biwa/commit/54b5bee61e48f8cb3be7210d95dcc6e3312b9c04))
- *(deps)* Lock file maintenance ([#565](https://github.com/risu729/biwa/pull/565)) - ([2631bf1](https://github.com/risu729/biwa/commit/2631bf110fc997769fe42d6963053ff9ec14dda6))
- *(deps)* Update rust crate ctor to v1 ([#590](https://github.com/risu729/biwa/pull/590)) - ([9be6d6c](https://github.com/risu729/biwa/commit/9be6d6c650354d15b96682a7e762b8525c5aea7d))
- *(deps)* Update rust crate ctor to v0.10.0 ([#530](https://github.com/risu729/biwa/pull/530)) - ([b763caa](https://github.com/risu729/biwa/commit/b763caa962f5cc4a88a113f8e96f4f80b7251716))
- *(deps)* Lock file maintenance ([#531](https://github.com/risu729/biwa/pull/531)) - ([b33317c](https://github.com/risu729/biwa/commit/b33317ca66387f0c52a109656adf3e8d594a27f2))
- *(deps)* Update rust crate ctor to v0.9.1 ([#508](https://github.com/risu729/biwa/pull/508)) - ([8cf3701](https://github.com/risu729/biwa/commit/8cf3701038700a1b1717051a5129aea6b7629086))
- *(deps)* Lock file maintenance ([#495](https://github.com/risu729/biwa/pull/495)) - ([39bc7d1](https://github.com/risu729/biwa/commit/39bc7d1532dd33ad476013484463e9773bd7d535))
- *(deps)* Lock file maintenance ([#462](https://github.com/risu729/biwa/pull/462)) - ([76628ca](https://github.com/risu729/biwa/commit/76628ca447764c83d0c1a93ba5ddabd357ca17b9))
- *(deps)* Update rust crate insta to v1.47.2 ([#466](https://github.com/risu729/biwa/pull/466)) - ([ec432f6](https://github.com/risu729/biwa/commit/ec432f69c205a9cbeb096cf279cbd9aa723803a2))
- *(deps)* Update rust crate insta to v1.47.1 ([#460](https://github.com/risu729/biwa/pull/460)) - ([63b0492](https://github.com/risu729/biwa/commit/63b0492b5b8cddaa5799e38570a8bfbda8cbaa44))
- *(deps)* Update rust crate ctor to v0.8.0 ([#458](https://github.com/risu729/biwa/pull/458)) - ([928a07f](https://github.com/risu729/biwa/commit/928a07f89f8fb45b4e206b3e81b6a2d0d83cb5be))
- *(deps)* Update rust crate ctor to v0.7.0 ([#457](https://github.com/risu729/biwa/pull/457)) - ([61f8c63](https://github.com/risu729/biwa/commit/61f8c635bed38f950bbf216a07a45427386c2aac))
- *(deps)* Update rust crate insta to v1.47.0 ([#450](https://github.com/risu729/biwa/pull/450)) - ([afe64a8](https://github.com/risu729/biwa/commit/afe64a8e0955325d276b17a3c36799effc4dba25))
- *(deps)* Lock file maintenance ([#361](https://github.com/risu729/biwa/pull/361)) - ([6540b40](https://github.com/risu729/biwa/commit/6540b40613c5e9fdc6f4b3e0f937b5929a443b4f))
- *(deps)* Update rust crate tempfile to v3.27.0 ([#322](https://github.com/risu729/biwa/pull/322)) - ([7e848bd](https://github.com/risu729/biwa/commit/7e848bdd17da01a3ecede640e2de1f8001e82d8a))
- *(deps)* Update rust crate ctor to v0.6.3 ([#319](https://github.com/risu729/biwa/pull/319)) - ([50eefcb](https://github.com/risu729/biwa/commit/50eefcb75cef8ccae9c85521e5c1c9266bd4940d))
- *(deps)* Lock file maintenance ([#317](https://github.com/risu729/biwa/pull/317)) - ([59187b0](https://github.com/risu729/biwa/commit/59187b08d89a20251e1611c028e1495cc53bb636))
- *(deps)* Lock file maintenance ([#288](https://github.com/risu729/biwa/pull/288)) - ([ca1a8f9](https://github.com/risu729/biwa/commit/ca1a8f91c3df194147f756d594d1457ac22e00fa))
- *(deps)* Update rust crate ctor to 0.6.0 ([#283](https://github.com/risu729/biwa/pull/283)) - ([487609c](https://github.com/risu729/biwa/commit/487609c6b44c921b5b4f930efbcd7577e2cc403d))
- *(deps)* Lock file maintenance ([#221](https://github.com/risu729/biwa/pull/221)) - ([5336a2b](https://github.com/risu729/biwa/commit/5336a2bc876f6d3f04e8513b9c90aba00dd5df89))
- *(deps)* Update Cargo.lock ([#200](https://github.com/risu729/biwa/pull/200)) - ([f23a0d1](https://github.com/risu729/biwa/commit/f23a0d19402dd2e8834ab3119281628ad639313c))
- *(hk)* Migrate to shared hk-config v1.0.0 ([#772](https://github.com/risu729/biwa/pull/772)) - ([719d44e](https://github.com/risu729/biwa/commit/719d44e16ad024533b814cb6c06bbb6a17b3b18b))
- *(mise)* Update tools and lockfile ([#593](https://github.com/risu729/biwa/pull/593)) - ([0c35edf](https://github.com/risu729/biwa/commit/0c35edf3ead30e05f34d357be1a4e4facf19a055))

