# Changelog

## [Unreleased]

## [0.1.1](https://github.com/risu729/biwa/compare/v0.1.0...v0.1.1)

### ⛰️ Features


- *(config)* Deserialize umask in config as octal string ([#331](https://github.com/risu729/biwa/pull/331)) - ([04b41a7](https://github.com/risu729/biwa/commit/04b41a7d7b90c6573ae0254d78185c0ca6ee0a97))
- *(env)* Support transferring and setting remote env vars ([#339](https://github.com/risu729/biwa/pull/339)) - ([486951a](https://github.com/risu729/biwa/commit/486951ac7a759611f7db7c0bd8e43c7ba2f24be3))
- Incorporate hostname into unique project name calculation to prevent cross-machine collisions. ([#265](https://github.com/risu729/biwa/pull/265)) - ([3dcfad5](https://github.com/risu729/biwa/commit/3dcfad54b204cfa9efe114fb880ee88f8affd4cb))
- Implement SSH file synchronization ([#257](https://github.com/risu729/biwa/pull/257)) - ([260f05a](https://github.com/risu729/biwa/commit/260f05a2236573e2ecf40474d0564904ec85a71e))
- Native SSH command execution with async-ssh2-tokio ([#250](https://github.com/risu729/biwa/pull/250)) - ([dab84f9](https://github.com/risu729/biwa/commit/dab84f95d174ef7fb80af7e754a162a0376dabfe))
- Add autocompletion feature and cli reference ([#244](https://github.com/risu729/biwa/pull/244)) - ([95e0375](https://github.com/risu729/biwa/commit/95e0375878bde64223b7c78161338f2d14bfe982))
- Comprehensive Configuration Resolution ([#237](https://github.com/risu729/biwa/pull/237)) - ([82ef9d8](https://github.com/risu729/biwa/commit/82ef9d87e978ab0c97161587c25dbc123acb8b84))

### 🐛 Bug Fixes


- *(cli)* Buffer configuration load logs ([#390](https://github.com/risu729/biwa/pull/390)) - ([2035e35](https://github.com/risu729/biwa/commit/2035e35b7051fcc06cbd8e43fb532e9eacc41255))
- *(config)* Error on missing required ssh settings ([#397](https://github.com/risu729/biwa/pull/397)) - ([a9f04d6](https://github.com/risu729/biwa/commit/a9f04d6baaec4576cab2cc35a75c5140f7bbdc75))
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
- *(run)* Forward stdin to remote commands ([#400](https://github.com/risu729/biwa/pull/400)) - ([b89e9e7](https://github.com/risu729/biwa/commit/b89e9e7a80f6b9e8581b8af865195413209b9c18))
- *(schema)* Mark fields with defaults as optional in JSON schema ([#332](https://github.com/risu729/biwa/pull/332)) - ([5409e96](https://github.com/risu729/biwa/commit/5409e96656561e48bb9e727b9c83d089de2d2ae0))
- *(ssh)* Drain silent command output ([#375](https://github.com/risu729/biwa/pull/375)) - ([474d798](https://github.com/risu729/biwa/commit/474d798d22d2f9de74db91323cc9e22d2d8c271a))
- *(sync)* Synchronize empty directories ([#378](https://github.com/risu729/biwa/pull/378)) - ([93a28e7](https://github.com/risu729/biwa/commit/93a28e7bb947dd3842900bf50be9c9aedd88de69))
- *(sync)* Check file limit before SSH connect ([#377](https://github.com/risu729/biwa/pull/377)) - ([453dfe9](https://github.com/risu729/biwa/commit/453dfe9b9c25cbde16bf9dfbe3448530c33c3633))
- Unify behavior of implicit biwa execution and biwa run ([#329](https://github.com/risu729/biwa/pull/329)) - ([dae941d](https://github.com/risu729/biwa/commit/dae941de9258ee79621ab84b497f24437dc0da21))
- Hide internal traces from end-user errors ([#327](https://github.com/risu729/biwa/pull/327)) - ([a9450ba](https://github.com/risu729/biwa/commit/a9450ba8be61e1427dc02676a569179bc3d6beab))

### ⚡ Performance


- *(ssh)* Reuse single SSH connection for sync and run ([#412](https://github.com/risu729/biwa/pull/412)) - ([d1da911](https://github.com/risu729/biwa/commit/d1da9117a15628e9b05077d88b785f81a886cc8d))

### 🚜 Refactor


- *(cli)* Remove log config and deferred loading ([#398](https://github.com/risu729/biwa/pull/398)) - ([7c5dd95](https://github.com/risu729/biwa/commit/7c5dd952679cd58ca59c3ac0e0d579e9fe7db0c3))
- *(config)* Move check_remote_root to config validation phase ([#379](https://github.com/risu729/biwa/pull/379)) - ([95192fb](https://github.com/risu729/biwa/commit/95192fbfb72215985aa47716b9f0c8bfe4a7fa4a))
- *(config)* Migrate from figment to confique ([#247](https://github.com/risu729/biwa/pull/247)) - ([eaa3693](https://github.com/risu729/biwa/commit/eaa369384ff6a4e0652a5a75f68cb494ae9444b0))
- *(ssh)* Replace async-ssh2-tokio with custom russh client wrapper ([#407](https://github.com/risu729/biwa/pull/407)) - ([6697920](https://github.com/risu729/biwa/commit/669792080998b8777b514270044e1e0e6a6c31ee))
- Migrate to color-eyre for improved error reporting ([#258](https://github.com/risu729/biwa/pull/258)) - ([ca16e0b](https://github.com/risu729/biwa/commit/ca16e0b84fc77cf56c1051b2760d90b7b2f919f0))

### 📚 Documentation


- Initialize docs with VitePress and Cloudflare Workers ([#213](https://github.com/risu729/biwa/pull/213)) - ([2ba4ebc](https://github.com/risu729/biwa/commit/2ba4ebc052e24855625104bd6ba1d54a9014186c))

### 🧪 Testing


- *(auth)* Remove unsafe env mutation in try_agent tests ([#382](https://github.com/risu729/biwa/pull/382)) - ([4f27ded](https://github.com/risu729/biwa/commit/4f27ded19131f3f7ca7d553219034c5e22785755))
- Use pretty_assertions ([#260](https://github.com/risu729/biwa/pull/260)) - ([e165a3d](https://github.com/risu729/biwa/commit/e165a3d9f103dbf82d190255fb8cc1fc1e0e0085))

### 🎨 Styling


- *(cargo)* Sort Cargo.toml ([#252](https://github.com/risu729/biwa/pull/252)) - ([769c839](https://github.com/risu729/biwa/commit/769c839c02a7237ebde2a644487b431c78b07bbf))

### 🧹 Chore


- *(clippy)* Enable nursery/restiction rules ([#253](https://github.com/risu729/biwa/pull/253)) - ([61545e7](https://github.com/risu729/biwa/commit/61545e77d68cbeb09679380707d5602b730a75a0))
- *(deps)* Lock file maintenance ([#361](https://github.com/risu729/biwa/pull/361)) - ([6540b40](https://github.com/risu729/biwa/commit/6540b40613c5e9fdc6f4b3e0f937b5929a443b4f))
- *(deps)* Update rust crate tempfile to v3.27.0 ([#322](https://github.com/risu729/biwa/pull/322)) - ([7e848bd](https://github.com/risu729/biwa/commit/7e848bdd17da01a3ecede640e2de1f8001e82d8a))
- *(deps)* Update rust crate ctor to v0.6.3 ([#319](https://github.com/risu729/biwa/pull/319)) - ([50eefcb](https://github.com/risu729/biwa/commit/50eefcb75cef8ccae9c85521e5c1c9266bd4940d))
- *(deps)* Lock file maintenance ([#317](https://github.com/risu729/biwa/pull/317)) - ([59187b0](https://github.com/risu729/biwa/commit/59187b08d89a20251e1611c028e1495cc53bb636))
- *(deps)* Lock file maintenance ([#288](https://github.com/risu729/biwa/pull/288)) - ([ca1a8f9](https://github.com/risu729/biwa/commit/ca1a8f91c3df194147f756d594d1457ac22e00fa))
- *(deps)* Update rust crate ctor to 0.6.0 ([#283](https://github.com/risu729/biwa/pull/283)) - ([487609c](https://github.com/risu729/biwa/commit/487609c6b44c921b5b4f930efbcd7577e2cc403d))
- *(deps)* Lock file maintenance ([#221](https://github.com/risu729/biwa/pull/221)) - ([5336a2b](https://github.com/risu729/biwa/commit/5336a2bc876f6d3f04e8513b9c90aba00dd5df89))
- *(deps)* Update Cargo.lock ([#200](https://github.com/risu729/biwa/pull/200)) - ([f23a0d1](https://github.com/risu729/biwa/commit/f23a0d19402dd2e8834ab3119281628ad639313c))

