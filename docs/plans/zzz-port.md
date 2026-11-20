# ZZZ — plano de desenvolvimento

> **Nota (2026-11-19): o repositório foi reorganizado depois deste plano.**
> O studio virou o crate `crates/studio` (antes `src/studio/`, incluído no
> web por `#[path]`), o web está em `crates/web` (antes `web/`), o relay em
> `crates/relay` (antes `web/relay/`), e tudo é um workspace Cargo só. As
> fases abaixo citam os caminhos e comandos de quando foram feitas (como
> `--features gui`); a estrutura atual está em
> [../architecture.md](../architecture.md) e os comandos em
> [../development.md](../development.md).

**ZZZ** é a versão web do ZigZag: o mesmo studio (egui) compilado para WebAssembly,
rodando no navegador, dentro deste fork (`quilombodigital/zigzag`).

Este plano é um documento vivo. Cada fase termina com um critério de aceite;
o que for descoberto no caminho entra em [Registro de mudanças](#registro-de-mudanças)
e ajusta as fases seguintes.

**Estado:** Fases 0 e 1 concluídas; Fases 2, 3, 5 e 6 feitas e a 7 pronta para o push — o studio abre no navegador, guarda o trabalho entre visitas, salva em arquivos de verdade, conversa com a máquina simulada e aceita agentes; falta testar com uma Skitch real e publicar (ver "Andamento" de cada fase).
O levantamento abaixo vem da leitura do código (upstream 0.1.5, commit
`92bfe1a`); o que a Fase 0 confirmou está anotado lá. O restante marcado como
*a verificar* continua sendo hipótese.

---

## 1. Decisões já tomadas

| # | Decisão | Consequência |
|---|---|---|
| D1 | Compilar o studio egui existente para WASM, não reescrever a UI | Paridade com o desktop; app em canvas (sem DOM) |
| D2 | Pasta nova `web/` **mais** ajustes pequenos em `src/` | Um único studio; ajustes protegidos por `cfg(target_arch = "wasm32")` |
| D3 | Trabalho fica só no fork, sem PR para o upstream | Liberdade para refatorar; custo é o merge a cada release do upstream |
| D4 | Envio à Brother Skitch (PP1) por Web Bluetooth já na primeira versão | Só Chrome/Edge; exige tornar a camada de máquina assíncrona |
| D5 | MCP via relay local planejado (não na primeira versão) | Binário nativo pequeno + WebSocket com a aba |

| D6 | Interface em inglês, como no upstream | Nenhum trabalho de tradução; textos novos da web também em inglês |
| D7 | Suporte só a Chrome e Edge (desktop) | Firefox e Safari recebem um aviso de navegador não suportado; sem caminhos alternativos para eles |
| D8 | A versão web se chama **ZZZ** (alusão ao "WWW") | Título da página, tela de carregamento e "About" usam "ZZZ", com crédito e link para o ZigZag original |
| D9 | Arquivo de teste de importação: `sample1.pes` (PES v1, 75 KB, fornecido pelo usuário) | Entra nos roteiros de aceite das Fases 1 e 2; copiar para `web/tests/fixtures/` se a licença do arquivo permitir |

Suposições ainda não confirmadas (ver [Perguntas em aberto](#11-perguntas-em-aberto)):
hospedagem no GitHub Pages, branch `web`.

**Efeitos de D7 no restante do plano:** File System Access API e Web Bluetooth
podem ser tratados como sempre presentes. O download como forma de salvar e a
Local Font Access API deixam de ser "alternativa" e "opcional": salvar usa
sempre o handle do arquivo, e as fontes instaladas no computador podem ser a
fonte principal de fontes, com as embutidas como base mínima. A página testa
as APIs necessárias no boot e, se faltar alguma, mostra o aviso em vez do app.

**Efeitos de D8:** o nome aparece só na camada web (`index.html`, título,
"About"). Nomes internos — crates, identificadores, o formato dos ids no SVG —
continuam "zigzag", para os arquivos seguirem compatíveis com o desktop e os
merges do upstream não conflitarem.

---

## 2. O que existe hoje

### 2.1 Estrutura

Um único pacote Cargo `zigzag` com uma lib e três binários:

| Alvo | Arquivo | Papel |
|---|---|---|
| lib `zigzag` | `src/lib.rs` | Motor: SVG → pontos → formatos de máquina |
| bin `zigzag` | `src/main.rs` | CLI |
| bin `zigzag-mcp` | `src/mcp_bridge.rs` | Ponte stdio ↔ HTTP para clientes MCP |
| bin `zigzag-studio` | `src/studio/main.rs` | Editor (feature `gui`) |

Tamanho: ~85 mil linhas no total, das quais ~54 mil em `src/studio/` (60 módulos
planos, sem subpastas, todos declarados com `mod` em `src/studio/main.rs` e
referenciados entre si como `crate::app::Studio`, `crate::canvas::...`).

### 2.2 Motor (fora de `src/studio/`)

Pontos de contato com o sistema, todos pequenos:

| Onde | O quê | Na web |
|---|---|---|
| `src/document/mod.rs:316` `Document::open(path)` | Lê o arquivo por caminho e decide por extensão (SVG, PES/DST/EXP/JEF, imagem) | Precisa de uma entrada por bytes |
| `src/document/mod.rs:512` `Document::save(path)` | `std::fs::write` | Já existe `to_svg()`; salvar vira responsabilidade da plataforma |
| `src/svg.rs:134` | `std::fs::read` | Idem |
| `src/svg.rs:138` `fontdb()` | `load_system_fonts()` num `OnceLock` imutável | Sem fontes no navegador; precisa de fontes carregadas e de um banco mutável |
| `src/machine/session.rs:86,185` | `std::thread::sleep` | Não existe em WASM |
| `src/machine/sim.rs` | `Instant::now`, `thread::sleep` | Idem |
| `src/machine/skitch.rs` | `btleplug` + `tokio` (feature `ble`) | Substituído por Web Bluetooth |
| `src/design.rs:1278` `caught()` | `catch_unwind` protege o planejador (8 usos) | Em `wasm32-unknown-unknown` um panic aborta; ver risco R3 |

O que já está pronto para a web:

- `formats::write_format(design, ext) -> Vec<u8>` devolve bytes, não escreve arquivo.
- `Document::from_svg(xml, name, path)` já constrói a partir de texto.
- `vectorcraft-trace` só usa threads se `available_parallelism() > 1`; em WASM
  cai sozinho no caminho de uma thread (`vendor/vectorcraft-trace/src/lib.rs:1120`).
- Dependências do motor (`usvg`, `resvg`, `i_overlay`, `roxmltree`, `kurbo`,
  `image`, `ttf-parser`, `serde_json`, `base64`) são Rust puro — *a verificar*
  na Fase 0, em especial as features de fontes do `usvg`/`fontdb`.

### 2.3 Studio (`src/studio/`)

| Tema | Situação | Arquivos principais |
|---|---|---|
| Renderização | egui 0.37 sobre glow. O shader de pontos já tem variante `#version 300 es` (`gpu.rs:204`), que é WebGL2. Há um caminho alternativo sem GPU (`canvas::draw_design_cached`) | `gpu.rs`, `canvas.rs` |
| Arquivos de configuração | Todos passam por `dock::config_file(nome)`: preferências, layout dos painéis, paletas, máquinas, sessão | `dock.rs:253`, `settings.rs`, `palettes.rs`, `machines.rs`, `session.rs` |
| Recuperação de sessão | `session.json` + `recovery.svg` por aba, gravação atômica com `rename` | `session.rs` (37 usos de `fs`) |
| Diálogos de arquivo | `rfd`; no macOS já usa o caminho assíncrono com `poll_file_dialog` num frame posterior | `file_dialog.rs` |
| Arrastar e soltar | Lê `dropped_files` por caminho | `app.rs:1283` |
| Threads | Trace de imagem, worker da máquina, servidor MCP, agente, atualizador, varredura de fontes | `trace_ui.rs:211`, `machine.rs:169`, `mcp.rs:178`, `agent.rs`, `update.rs`, `main.rs:105` |
| Relógio | `Instant::now` / `SystemTime::now` em 16 arquivos | vários |
| Máquina | Worker bloqueante numa thread, comandos por `mpsc`, estado em `Arc<Mutex<Snapshot>>` | `machine.rs`, `sew.rs` |
| MCP | Servidor HTTP `tiny_http` em 127.0.0.1:7878; cada `tools/call` vai para a thread da UI por canal. `mcp::handle_rpc` é independente do transporte | `mcp.rs`, `tools.rs` (33 ferramentas) |
| Agente embutido | Lança o CLI do Claude Code ou Codex como processo filho | `agent.rs`, `agent_log.rs` |
| Só desktop | Atualizador, log de crash, menu nativo do macOS, ícone do app, captura de telas para o README, benchmarks | `update.rs`, `crash.rs`, `native_menu.rs`, `app_icon.rs`, `capture.rs`, `perf_bench.rs` |
| Janela | `ViewportCommand::{Title, Close, StartDrag, Maximized}` | `app.rs:1451`, `chrome.rs`, `commands.rs:711`, `tabs.rs:757` |
| Testes | 30 arquivos usam `egui_kittest` | — |

Dos 177 usos de `std::fs` no studio, boa parte está em testes (`cfg(test)`,
com `temp_dir()`), que não compilam no build web. O número real a tratar sai
da Fase 0.

---

## 3. Arquitetura alvo

```
zigzag/
  Cargo.toml              passa a ser também raiz de workspace
  src/                    motor + studio (ajustes pontuais)
    studio/platform/      NOVO: tudo que difere entre desktop e web
      mod.rs              API comum
      native.rs           std::fs, threads, rfd
      web.rs              IndexedDB, File API, spawn_local, Web Bluetooth
  web/                    NOVO
    Cargo.toml            crate zigzag-web (cdylib)
    src/lib.rs            ponto de entrada WASM; inclui os módulos do studio
    index.html            página, canvas, tela de carregamento, erro fatal
    Trunk.toml            build
    assets/fonts/         fontes embutidas (licença OFL)
    relay/                Fase 6: binário zigzag-mcp-relay
    tests/                testes de navegador
    PLAN.md               este arquivo
  .github/workflows/web.yml   build + deploy no GitHub Pages
```

### 3.1 Como o crate web enxerga o studio

O studio é um binário; seus módulos usam caminhos `crate::...`, então precisam
estar na raiz do crate que os compila. Um pacote só pode ter uma lib e a do
pacote raiz já é o motor.

**Escolha inicial:** `web/src/lib.rs` declara os mesmos módulos apontando para
os arquivos originais:

```rust
#[path = "../../src/studio/app.rs"]    mod app;
#[path = "../../src/studio/canvas.rs"] mod canvas;
// ...um por módulo, menos os que não existem na web
```

- Vantagem: `src/studio/main.rs` fica intocado; os módulos só-desktop
  simplesmente não são listados.
- Custo: a lista precisa acompanhar a de `main.rs`. Um teste simples no CI
  compara as duas e falha se um módulo novo do upstream não foi classificado
  (incluído na web ou explicitamente excluído).
- Alternativa, se `#[path]` der atrito: mover a lista de módulos para um
  `src/studio/lib.rs` e fazer `main.rs` e `web/` dependerem dele como um
  pacote `zigzag-studio`. Diff maior em `src/`, estrutura mais limpa.

O crate `zigzag-web` define suas próprias features e dependências
(`eframe` com as features web, `wasm-bindgen`, `web-sys`, `js-sys`,
`wasm-bindgen-futures`, `web-time`, `console_error_panic_hook`).

### 3.2 Módulo de plataforma

Um único lugar para tudo que difere. O resto do studio chama `platform::*` e
não sabe onde está rodando. Superfície prevista:

| API | Desktop | Web |
|---|---|---|
| `platform::time::{Instant, SystemTime}` | `std::time` | crate `web-time` |
| `platform::store::{read, write, remove, rename, exists}` | arquivos em `config_dir()` | mapa em memória espelhado em IndexedDB |
| `platform::files::pick_open(filtros)` | `rfd` → caminho → bytes | `<input type=file>` / File System Access API |
| `platform::files::save(nome, bytes, destino)` | `std::fs::write` | gravar no handle (FSA) ou download |
| `platform::task::spawn(trabalho, ao_terminar)` | `std::thread::spawn` | Fase 2: no próximo frame; Fase 4: Web Worker |
| `platform::open_url(url)` | `open` / `xdg-open` / `explorer` | `window.open` |
| `platform::window::{set_title, close}` | `ViewportCommand` | `document.title`; `close` não faz nada |
| `platform::ble::connect()` | `btleplug` | Web Bluetooth |
| `platform::fonts::load()` | fontes do sistema | embutidas + enviadas + Local Font Access |

**Armazenamento síncrono sobre IndexedDB.** O código do studio lê e grava
configuração de forma síncrona; IndexedDB é assíncrono. Solução: no boot, antes
de criar o app, carregar todas as chaves do IndexedDB para um mapa em memória
(o `main` web já é assíncrono). Leituras vêm do mapa; gravações atualizam o
mapa e disparam a persistência em segundo plano. Em `pagehide` /
`visibilitychange` força-se a gravação pendente. `localStorage` fica de fora
por causa do limite de ~5 MB (um `recovery.svg` com imagem embutida passa
disso).

### 3.3 Modelo de documentos na web

No desktop um documento tem um `PathBuf`. Na web não há caminhos. Proposta:

- Um documento aberto guarda uma **origem**: `Nenhuma` (novo), `Handle` (File
  System Access API, Chrome/Edge — permite "Salvar" de verdade no mesmo
  arquivo) ou `Importado` (só nome; "Salvar" vira download).
- Internamente, para mexer pouco no código existente, cada documento ganha um
  caminho virtual (`/docs/<id>/<nome>.svg`) usado só como identidade — títulos
  de aba, recentes, sessão. Os bytes vivem no `platform::store`.
- "Exportar" (PES, DST, EXP, JEF, VP3, folha de trabalho) sempre gera bytes
  com `formats::write_format` e entrega pela plataforma.
- Arrastar arquivos para a janela: no navegador o egui entrega `bytes` em vez
  de `path`; `app.rs:1283` passa a aceitar os dois.

### 3.4 Camada de máquina assíncrona

Hoje: `Transport` é uma trait bloqueante, `Session` dorme com `thread::sleep`,
e `studio/machine.rs::worker` é um laço numa thread. Web Bluetooth é baseado
em promises e só existe na thread principal — não dá para manter o modelo
bloqueante nem movê-lo para um Web Worker.

Mudança (afeta desktop e web, por isso tem fase própria):

1. `Transport` vira assíncrona (`async fn request/send/read`).
2. `Session` vira assíncrona; os `sleep` passam por `platform::time::sleep`.
3. `worker` vira `async fn`; o canal `mpsc` vira um canal assíncrono
   (`futures::channel::mpsc`).
4. Desktop: a thread `skitch` continua existindo e roda o worker com
   `block_on` (o runtime tokio que `BleLink` já cria). `BleLink` deixa de
   embrulhar cada chamada em `block_on`.
5. Web: `wasm_bindgen_futures::spawn_local(worker(...))`.
6. `Sim` implementa a trait assíncrona de forma trivial — continua sendo o
   alvo dos testes nos dois mundos.

`Snapshot` em `Arc<Mutex<_>>` continua igual (em WASM o mutex nunca disputa).

---

## 4. Fases

Tamanho relativo: **P** (até ~2 dias), **M** (~1 semana), **G** (várias
semanas). São estimativas de leitura de código, não medidas.

### Fase 0 — Ambiente e prova de viabilidade (P)

Objetivo: trocar hipóteses por uma lista real de erros de compilação.

1. Instalar: `rustup` (toolchain 1.95, exigida pelo `Cargo.toml`), target
   `wasm32-unknown-unknown`, `trunk`, `wasm-bindgen-cli`, `wasm-opt` (binaryen).
2. Confirmar que o desktop compila e os testes passam **antes** de tocar em
   nada (`cargo test`, `cargo build --features gui`). É a linha de base.
3. `cargo check --lib --target wasm32-unknown-unknown` no motor. Anotar erros.
4. Criar `web/` mínimo com um "hello egui" em `eframe::WebRunner` usando a
   mesma versão do eframe (0.37) e glow. Confirma toolchain e Trunk.
5. Incluir os módulos do studio com `#[path]` e rodar `cargo check`. Não
   precisa compilar — o objetivo é o inventário de erros, classificado em:
   relógio, arquivos, threads, dependência nativa, API de janela.
6. Medir o `.wasm` do "hello" e estimar o tamanho final.

**Aceite:** inventário de erros anexado a este plano; decisão confirmada ou
revista sobre `#[path]` (3.1); tamanhos reais das fases seguintes.

**Ponto de decisão:** se o inventário mostrar algo estrutural (por exemplo,
uma dependência do motor que não compila em WASM), parar e replanejar.

#### Resultado da Fase 0 (2026-11-18)

Ambiente: Rust 1.100 (stable, MSVC), target `wasm32-unknown-unknown`,
Trunk 0.22. Windows 10.

| Verificação | Resultado |
|---|---|
| Linha de base do desktop (`cargo test --features gui`) | Motor: 235 passam. Ponte MCP: 3 passam. Studio: 265 passam, **1 falha que já existia** — `agent::tests::runs_an_agent_program_and_streams_its_output` usa um script `#!/bin/sh` como programa falso e não roda no Windows. Não tem relação com a web (o módulo `agent` fica fora). |
| Motor em WASM (`cargo check --lib --target wasm32-unknown-unknown`) | **Compila sem nenhum erro.** R1 afastado para o motor. Único aviso: `build.rs` tenta gerar o recurso de ícone do Windows; inofensivo, calar na Fase 1. |
| Hello egui + motor (`web/`, eframe 0.37 glow) | Compila, e **roda no navegador** (`trunk serve`): a página abre em WebGL2 e o motor planeja um círculo de teste (1.377 pontos) dentro da aba, sem erros no console. |
| Tamanho do `.wasm` do hello (release, `wasm-opt -O2`) | 5,7 MB; 2,2 MB com gzip. Já inclui o motor inteiro e o egui; falta o código do studio. Estimativa para o app completo: 10–15 MB, 4–6 MB comprimido — a medir na Fase 2. |
| Módulos do studio via `#[path]` (feature `studio`) | A técnica funciona: 51 módulos incluídos, 8 deixados de fora. **59 erros**, todos em categorias previstas (tabela abaixo). |

Inventário dos 59 erros de compilação do studio em WASM:

| Categoria | Erros | Onde | Tratamento |
|---|---|---|---|
| Referências aos módulos só-desktop (`agent`, `agent_log`, `update`, `crash`, `capture`, `native_menu`, `app_icon`) | 36 | `app.rs` (a maioria), `chrome.rs`, `commands.rs`, `dock.rs`, `settings.rs`, `mcp_chip.rs` | `cfg` nos pontos de chamada, ou versões vazias desses módulos na web |
| Servidor HTTP `tiny_http` | 10 | `mcp.rs` | Separar o servidor (desktop) de `handle_rpc` e dos tipos (comuns) |
| Contexto GL não é `Send`/`Sync` em WASM | 11 (um único ponto) | `gpu.rs:452`, o `CallbackFn` que captura `Arc<Mutex<GlState>>` | Invólucro `Send + Sync` só para WASM (uma thread) |
| `rfd::FileDialog` bloqueante não existe em WASM | 1 | `file_dialog.rs:76` | Só o caminho assíncrono na web |
| Chamadas MCP na UI (`run_mcp_call`, `log_call`) | 2 | `app.rs:1266` | Vêm de `agent_log`/`tools`; resolver junto com a primeira linha |

O que **não** apareceu, e por quê: `std::fs`, `std::thread`, `Instant::now` e
`std::process` compilam em `wasm32-unknown-unknown` — falham só ao executar
(erro "unsupported" ou panic). Esses pontos continuam precisando do módulo
`platform` (Fase 2), mas o compilador não vai apontá-los: precisam ser
achados por busca e por teste no navegador. Rever a lista da seção 2.3 quando
a Fase 2 começar.

Outras observações:

- Dependências extras que o crate web precisa declarar por usar os módulos do
  studio direto: `usvg`, `resvg`, `base64` (já declaradas) e `rfd` (entra na
  Fase 2, junto com os diálogos).
- Para rodar: `trunk serve --release` dentro de `web/`. Para o inventário:
  `cargo check --target wasm32-unknown-unknown --features studio`.
- `web-time` já está na árvore de dependências (o egui usa).
- O crate `web/` está, por ora, fora de workspace, com `Cargo.lock` e `target/`
  próprios (o lock foi copiado da raiz para manter as mesmas versões). Assim o
  desktop não foi tocado: **nenhum arquivo de `src/` foi alterado na Fase 0.**
- Pode haver uma segunda leva de erros depois que estes forem resolvidos.

**Decisões confirmadas:** `#[path]` (3.1) segue como a escolha. Tamanhos das
fases mantidos; a Fase 2 começa pelos 59 erros acima.

### Fase 1 — Motor em WASM (P/M)

1. `Document::open_bytes(nome, extensão, bytes)`; `Document::open(path)` passa
   a ler o arquivo e chamar essa função. Mesmo tratamento em `svg.rs`.
2. `Document::save` fica só no desktop (`cfg(not(wasm32))`); a web usa
   `to_svg()` + plataforma.
3. Banco de fontes: trocar o `OnceLock<Arc<Database>>` por algo recarregável
   (`RwLock<Arc<Database>>` com contador de geração), para aceitar fontes que
   chegam depois do boot. No desktop o comportamento não muda.
4. Relógio do motor (`design.rs:3313`, `lettering.rs:1206`, `sim.rs`): usar
   `web-time`.
5. Teste em WASM (`wasm-bindgen-test`, rodando em Node ou navegador headless):
   abrir cada SVG de `examples/`, planejar, exportar PES e DST.

**Aceite:** `cargo check --lib --target wasm32-unknown-unknown` limpo; testes
do desktop continuam passando; o teste WASM exporta os exemplos.

**Comparação com o nativo:** exportar os mesmos exemplos com o CLI nativo e
comparar. Não exigir igualdade byte a byte de início — `sin`/`cos` vêm de
implementações diferentes no nativo e no WASM e podem divergir no último bit,
deslocando um ponto por arredondamento. Comparar número de pontos, cores e
caixa envolvente, com tolerância; investigar se a diferença for maior que isso.

#### Resultado da Fase 1 (2026-11-18)

Arquivos de `src/` alterados (lista a manter, ver seção 9):

| Arquivo | Mudança | Efeito no desktop |
|---|---|---|
| `src/document/mod.rs` | Novo `Document::open_bytes(nome, bytes, caminho)`; `open(path)` lê o arquivo e chama essa função | Nenhum: mesma lógica, mesmas mensagens |
| `src/svg.rs` | Banco de fontes atrás de um `RwLock`; novos `add_fonts(bytes)` e `fonts_generation()` | Nenhum: as fontes do sistema continuam carregadas uma vez |
| `src/machine/sim.rs` | `Instant` vem de `web-time` | Nenhum: no desktop é o `Instant` do `std` |
| `Cargo.toml` / `Cargo.lock` | Dependência `web-time` (já estava na árvore, via egui) | Nenhum |
| `build.rs` | Não gera o recurso de ícone quando o alvo não é Windows | Nenhum; some o aviso no build WASM |

Não foi preciso mexer em: `Document::save` e `svg::load_file` (compilam em
WASM e só não são chamados lá); o relógio de `design.rs` e `lettering.rs`
(está só em testes). Os `thread::sleep` de `session.rs` e `sim.rs` ficam para
a Fase 5, junto com a camada assíncrona.

Testes novos em `web/tests/engine.rs` — 8 testes que rodam iguais no desktop e
em WASM (Node, via `wasm-bindgen-test-runner`): abrir, planejar, exportar nos
cinco formatos, reabrir o PES; os quatro SVG de `examples/`, uma imagem
traçada, um arquivo recusado, letras com uma fonte entregue por bytes e
(opcional, fora do repositório) o `sample1.pes`.

| Verificação | Resultado |
|---|---|
| Testes do motor em WASM | 8 de 8 passam |
| Mesmos testes no desktop | 8 de 8 passam |
| Saída WASM × desktop | **Idêntica byte a byte** em 6 dos 7 desenhos, nos cinco formatos — inclusive o `bench-grid` (213 mil pontos), a imagem traçada e o `sample1.pes`. Em `lettering.svg` os arquivos têm o mesmo tamanho e o mesmo número de pontos, mas bytes diferentes: é o desenho com arcos, onde a trigonometria do WASM e a do Windows divergem no último bit, como previsto |
| `sample1.pes` | Abre nos dois: 11.807 pontos, 11 cores, 63,8 × 95,8 mm; reexporta nos cinco formatos |
| Trace de imagem em WASM | Funciona (decodificar PNG, traçar, planejar) |
| Fontes por bytes em WASM | Funciona: sem fontes do sistema, `add_fonts` + letras bordadas |
| Testes do desktop (`cargo test --features gui`) | Igual à linha de base: 503 passam, a mesma falha pré-existente |
| `cargo check` do motor em WASM | Limpo, sem avisos |

Como rodar, dentro de `web/`:

```sh
cargo test --test engine -- --nocapture                                   # desktop
cargo test --target wasm32-unknown-unknown --test engine -- --nocapture   # WASM (Node)
```

Com `RUSTFLAGS="--cfg zzz_local"` e um PES em
`web/tests/fixtures/local/sample1.pes` (pasta ignorada pelo git), roda também
o teste do arquivo externo. Requer `wasm-bindgen-cli` na mesma versão do
crate (0.2.129).

Pendências que passam para a Fase 2:

- `add_fonts` substitui o banco inteiro; documentos já abertos continuam com
  as fontes com que foram lidos. O studio precisa reler as listas de fontes
  quando `fonts_generation()` mudar.
- Tempo de planejamento em WASM ainda não foi medido contra o nativo (os
  tempos do teste em Node não servem de medida: build de depuração).
- A comparação WASM × desktop foi feita à mão; automatizar no CI (Fase 7),
  exigindo igualdade nos desenhos sem arcos e tolerância nos demais.

### Fase 2 — Studio abre no navegador (M/G)

A maior fase. Meta: **abrir um SVG, editar, exportar um arquivo de máquina.**

1. **`platform` (3.2)** com as implementações nativa e web de: relógio,
   armazenamento, arquivos, tarefas, `open_url`, janela.
2. **Trocas mecânicas** no studio: `std::time::Instant` → `platform::time`;
   `std::fs` de configuração → `platform::store`; `sew::open_url` → plataforma.
3. **Módulos fora da web** (não listados em `web/src/lib.rs`, com os pontos de
   chamada protegidos por `cfg`): `update`, `crash`, `native_menu`, `app_icon`,
   `capture`, `perf_bench`, `agent`, `agent_log`, o servidor de `mcp`
   (`handle_rpc` e os tipos ficam). `machine`/`sew` ficam compilando mas com a
   conexão desabilitada até a Fase 5 — o simulador pode funcionar antes.
4. **Entrada web** (`web/src/lib.rs`): carregar o armazenamento do IndexedDB,
   carregar as fontes embutidas, instalar o hook de panic, iniciar
   `eframe::WebRunner` no canvas.
5. **Diálogos de arquivo:** reaproveitar o caminho assíncrono que o macOS já
   usa (`poll_file_dialog`). O resultado deixa de ser `PathBuf` e passa a ser
   "nome + bytes" (abrir) ou "destino" (salvar). Primeira versão: abrir por
   seletor, salvar/exportar por download.
6. **Arrastar e soltar** por bytes.
7. **Trace de imagem** sem thread: mostra "traçando…", roda no frame seguinte.
   A interface congela durante o trace; resolvido na Fase 4.
8. **Janela:** título vira `document.title`; esconder a barra de título
   própria (`chrome.rs`) e os itens "Sair"/"Fechar janela"; menus desenhados
   na janela, como já é no Windows e no Linux.
9. **GPU:** validar o renderizador de pontos em WebGL2. Se falhar em algum
   navegador, cair no caminho sem GPU que já existe.
10. **Página** (`index.html`): tela de carregamento com progresso do download
    do WASM; mensagem clara quando o navegador não tem WebGL2; tela de erro
    fatal com botão "recarregar" (ver R3).
11. **Atalhos de teclado:** revisar `commands.rs`. O navegador não entrega
    Ctrl+W, Ctrl+T, Ctrl+N e Ctrl+Tab; Ctrl+S, Ctrl+O e Ctrl+P precisam de
    `preventDefault`. Definir atalhos alternativos na web onde houver conflito.
12. **Sair com alterações:** `beforeunload` avisa quando há documento não salvo.

**Aceite:** em Chrome e Edge, a partir de `trunk serve`: abrir
`examples/badge.svg`, mover um objeto, mudar um tipo de ponto, desfazer,
exportar PES e abrir o PES exportado de volta. `cargo test` do desktop
continua passando.

#### Andamento da Fase 2 (2026-11-19)

**Marco atingido:** o studio inteiro compila para WASM e abre no navegador.
Verificado em `trunk serve`: a interface desenha (ferramentas, painéis,
bastidor), `badge.svg` e `sample1.pes` abrem ao serem soltos na janela, os
pontos são desenhados pelo renderizador de GPU (WebGL2), e "Save As" entrega
o SVG como download. Sem erros no console.

Duas mudanças de rumo em relação ao que este plano dizia, ambas para
diminuir o que se altera em `src/`:

1. **Nenhum módulo do studio ficou de fora** (só `perf_bench`, que é teste).
   Os módulos só-desktop (`agent`, `update`, `crash`, `capture`,
   `native_menu`, `app_icon`, o servidor de `mcp`) compilam em WASM como
   estão — `std::process`, `std::net` e `std::thread` existem lá e só falham
   se forem chamados. Em vez de 36 pontos de `cfg` para excluí-los, bastou
   impedir que sejam alcançados: 3 pontos. Custo: código morto no `.wasm`.
2. **`zigzag::vfs` no lugar do `platform::store`** (seção 3.2). Um módulo no
   motor com as funções de arquivo (`read`, `write`, `rename`…): no desktop
   são as de `std::fs`; no navegador, arquivos em memória por caminho. Com
   isso o código que abre, salva e guarda configuração por caminho roda sem
   mudança, e o modelo de documentos da seção 3.3 (caminhos virtuais) saiu de
   graça: um arquivo escolhido ou solto é gravado em `/files/<nome>` e aberto
   como qualquer outro. A troca foi mecânica: 39 chamadas `std::fs::` →
   `vfs::`, só fora dos testes.

O que é específico do navegador ficou em `web/src/platform.rs` (download,
arquivos soltos) e em poucos `cfg(target_arch = "wasm32")` no studio.

Arquivos de `src/` tocados nesta fase:

| Arquivo | Mudança |
|---|---|
| `src/vfs.rs` (novo), `src/lib.rs` | O módulo de arquivos, com teste |
| `src/document/mod.rs`, `src/svg.rs` | `std::fs` → `vfs` |
| `src/studio/{app,dock,machine,machines,palettes,session,settings,theme,tools,trace_ui}.rs` | `std::fs` → `vfs` |
| `src/studio/{app,canvas,dock,lettering,machine,mcp,mcp_chip,planner,profile,agent_log}.rs` | `std::time::Instant` → `web_time::Instant` |
| `src/studio/file_dialog.rs` | Caminho web: seletor do navegador para abrir; salvar/exportar grava em memória e entrega por download |
| `src/studio/gpu.rs` | `unsafe impl Send for GlState` em WASM (uma thread só) |
| `src/studio/planner.rs` | Em WASM o planejamento nunca vai para a thread de fundo |
| `src/studio/trace_ui.rs` | Em WASM o trace roda no próprio frame |
| `src/studio/app.rs` | Atualizador e log de crash desligados em WASM; arquivos soltos lidos de forma assíncrona |
| `src/studio/agent.rs` | Em WASM não procura os programas de agente (criava uma thread) |
| `src/studio/session.rs`, `src/studio/dock.rs` | Gravação atômica e pasta de configuração (`/zigzag`) em WASM |

Medidas: `.wasm` de 11,1 MB, 4,1 MB com gzip (release, `wasm-opt -O2`) —
dentro da estimativa. Testes do desktop: iguais à linha de base, mais o teste
novo do `vfs`.

Testado só por automação até aqui (soltar arquivo e atalho de teclado
simulados); falta uma passada manual pela interface.

**Segunda rodada (2026-11-19)**, conferida com capturas da tela em tamanho
real (a página aceita `?keep-frame`, que guarda os pixels de cada frame para
leitura — é o que os testes de navegador vão usar):

- [x] O que é só do desktop não aparece: painel e botão do agente, "Ask
      Agent…", "Check for Updates…", "Show Crash Logs", "Quit". Uma lista só,
      `commands::desktop_only`, tira dos menus e desliga os atalhos.
- [x] Atalhos que o navegador guarda para si: "New" e "Close" ficam nos menus
      sem atalho (Ctrl+N e Ctrl+W nunca chegam à página).
- [x] Título da aba do navegador ("• badge — ZZZ"), e aviso do navegador ao
      sair com alterações não salvas.
- [x] Página: tela de carregamento, aviso de navegador (bloqueia sem
      WebGL 2; sem Web Bluetooth ou File System Access avisa e deixa abrir
      assim mesmo) e tela de erro com o motivo e "Reload".
- [x] Fontes mínimas: as duas que vêm com o egui (Ubuntu Light e Hack). A
      ferramenta de texto funciona: "ZZZ" digitado vira letras em cetim.
- [x] Conectar à máquina diz que ainda não existe nesta versão, em vez de
      procurar para sempre.
- [x] Links ("ZigZag on GitHub", "Report an Issue…") abrem em outra aba.
- [x] `Trunk.toml` observa `src/` e `vendor/`: `trunk serve` recompila e
      recarrega a página sozinho (cerca de 30 s por mudança).
- [x] Arquivos abertos aparecem pelo nome ("Opened badge.svg"), não por um
      caminho inventado.
- [x] Tempo de planejamento, release, Node × nativo:

      | Desenho | Nativo | WASM |
      |---|---|---|
      | `badge.svg` | 5,3 ms | 6,4 ms |
      | `groups.svg` | 3,9 ms | 6,4 ms |
      | `bench-grid.svg` (213 mil pontos) | 20,4 ms | 30,2 ms |
      | `lettering.svg` | 25,8 ms | 72,0 ms |
      | imagem traçada | 5,4 ms | 29,4 ms |

      Entre 1,2 e 5 vezes mais lento, e tudo abaixo de 75 ms: o risco R4 não
      se confirmou nos exemplos. O planejador em Web Worker (Fase 4) deixa de
      ser prioridade; o trace de imagem continua sendo o candidato.

**Falta na Fase 2:**

- [ ] Passada manual por uma pessoa: ferramentas de desenho, edição,
      desfazer, exportar PES/DST pelo menu, abrir pelo seletor de arquivo. Só
      foram exercitados por automação: abrir soltando o arquivo, selecionar,
      "Save As" pelo atalho, a ferramenta de texto e o menu File.
- [ ] A fonte da interface é a do egui (Ubuntu Light): os nomes dos menus
      ficam apertados. No desktop o studio procura Inter entre as fontes do
      sistema; na web depende das fontes da Fase 3.
- [ ] Ctrl+Shift+P ("Place SVG…"), Ctrl+O e Ctrl+S: conferir à mão que o
      navegador não fica com eles.
- [ ] A lista de painéis ainda oferece "Agent".
- [ ] `.wasm` leva código que nunca roda na web (agente, atualizador, servidor
      MCP). Medir quanto, e decidir se vale tirar.

O que os arquivos em memória ainda não fazem (Fase 3): persistir entre
visitas (IndexedDB, lendo `vfs::mem::take_changes`) e salvar no arquivo
original (File System Access API).

### Fase 3 — Persistência, arquivos de verdade e fontes (M)

1. **Sessão:** `session.json` e `recovery.svg` pelo `platform::store`;
   reabrir a página restaura as abas e as alterações não salvas.
2. **Preferências, layout, paletas, máquinas** persistem entre visitas.
3. **File System Access API** (Chrome/Edge): abrir devolve um handle; "Salvar"
   grava no mesmo arquivo; "Salvar como" e "Exportar" abrem o seletor nativo.
   Onde a API não existe, continua o download.
4. **Fontes:**
   - conjunto embutido pequeno (licença OFL), baixado como arquivo estático à
     parte, não dentro do WASM;
   - "Adicionar fonte…" envia um TTF/OTF, guardado no IndexedDB;
   - opcional: Local Font Access API (`queryLocalFonts()`, Chrome/Edge, com
     permissão) para usar as fontes instaladas no computador;
   - aviso ao abrir um SVG que pede uma fonte ausente, em vez de trocar em
     silêncio.
5. **Área de transferência:** copiar/colar SVG como texto funciona pelo egui;
   verificar colar imagem (para trace) — *a verificar* se o eframe web entrega.
6. **Cota de armazenamento:** tratar `QuotaExceededError` com mensagem, e
   pedir `navigator.storage.persist()`.

**Aceite:** fechar e reabrir a aba mantém o trabalho; em Chrome, Ctrl+S grava
no arquivo original; um texto com fonte embutida vira letras bordadas.

#### Andamento da Fase 3 (2026-11-19)

Feito e conferido no navegador (o seletor de arquivos do navegador foi
simulado nos testes: é um diálogo do sistema, que a automação não alcança):

- [x] **Persistência.** `web/src/files.js` guarda no IndexedDB os arquivos em
      memória. No início, `platform::restore` traz tudo de volta antes de o
      studio subir; a cada frame, e também a cada segundo e ao esconder a
      página, `platform::flush` envia o que foi escrito. Recarregar a página
      devolve as abas abertas ("Picked up where you left off: 2 documents"),
      as preferências e o layout.
- [x] **Abrir e salvar em arquivos de verdade** (File System Access API).
      Abrir guarda o "handle" do arquivo; "Save" grava de volta nele; "Save
      As" e "Export" abrem o seletor do navegador. Um documento que chegou
      sem handle (solto na janela, ou novo) pergunta onde salvar na primeira
      vez e lembra. Os handles também ficam no IndexedDB, então "Save"
      continua indo ao mesmo arquivo depois de recarregar (o navegador pede a
      permissão de novo). Sem a API, o caminho antigo: download.
- [x] **Exportações não ficam guardadas**: um PES ou DST é entregue e sai da
      memória; só documentos (SVG) e o que é do studio persistem.
- [x] **"Type › Add Fonts…"** (só na web): escolhe arquivos TTF/OTF/TTC, que
      passam a valer na hora e ficam guardados para as próximas visitas.
- [x] `navigator.storage.persist()` é pedido no início.

Como ficou a divisão: o studio continua trabalhando com caminhos
(`zigzag::vfs`); `platform.rs` decide o destino de cada escrita pelo caminho —
sob `/zigzag` é do studio (só persiste), fora dali é da pessoa (persiste e
vai para o arquivo no computador).

Em `src/` nesta fase: `vfs::mem::forget`; `file_dialog.rs` chama
`platform::{open_panel, save_panel}`; `app.rs` chama `platform::flush` a cada
frame; `commands.rs` ganhou `Action::AddFonts` e a lista `browser_only`.

**Fontes embutidas (2026-11-19):** Inter 4.1, Regular e Bold, do release
oficial (`rsms/inter`, licença SIL OFL 1.1, em `web/assets/fonts/` com a
licença ao lado). Embutidas no `.wasm` (`include_bytes!`), carregadas antes
do studio: ele já procura a Inter para a própria interface, como no desktop,
e ela vira também a fonte padrão das letras bordadas. `.wasm`: 11,9 MB,
4,5 MB com gzip (+0,8 MB e +0,4 MB). Os títulos não ficam em negrito: o
studio pede o eixo de peso de uma fonte variável, e a Inter embutida é a
estática (a variável tem o dobro do tamanho).

**Falta na Fase 3:**

- [x] **Biblioteca de fontes para bordar** (2026-11-19): 27 famílias do
      Google Fonts (`google/fonts`), licenças OFL e Apache, 5,5 MB, em
      `web/assets/fonts/library/` (lista e licenças no `README.md` de lá):
      sem serifa, com serifa, cursivas e em bloco/colegial. Ficam fora do
      `.wasm`: a página as busca em segundo plano depois que o studio abre,
      e o navegador as guarda em cache. Teste novo
      (`every_font_in_the_library_embroiders`, desktop e WASM): cada uma
      vira pontos em cetim. Com a Poppins presente, as letras novas começam
      nela, como no desktop (o studio procura Poppins antes de Inter).
- [x] A lista de fontes do painel de letras não se atualizava (fontes de
      "Add Fonts…" só apareciam depois de recarregar): agora segue
      `fonts_generation`.
- [ ] Fontes variáveis bordam só na instância padrão (Regular): o botão
      "Bold" não as engrossa. O mesmo vale para famílias de um peso só.
- [ ] Ficaram de fora por ora: Sacramento (traço fino demais para cetim) e
      Roboto Slab (não está com esse nome no repositório).
- [ ] **Fontes instaladas no computador** (Local Font Access API). Não dá
      para carregar todas de uma vez (centenas de arquivos); precisa de uma
      janela para escolher famílias, ou de carga sob demanda — as duas mexem
      na interface de texto. Por ora, "Add Fonts…" cobre o caso.
- [ ] Aviso ao abrir um SVG que pede uma fonte que não está carregada.
- [ ] Dois arquivos com o mesmo nome, de pastas diferentes, hoje são o mesmo
      arquivo para o ZZZ (a identidade é o nome). Resolver com um nome
      interno único por handle.
- [ ] Documentos abertos ficam no IndexedDB para sempre. Falta limpar os que
      não estão mais em nenhuma aba, e tratar o erro de cota cheia com uma
      mensagem (hoje só vai para o console).
- [ ] Colar imagem da área de transferência (para traçar): não verificado.
- [ ] Testar com o seletor de verdade, à mão: abrir, editar, Ctrl+S, conferir
      o arquivo no disco; exportar PES.

### Fase 4 — Desempenho (M)

1. Medir primeiro: tempo de planejamento e de trace nos exemplos, nativo ×
   WASM. Só otimizar o que aparecer.
2. **Web Worker** para trace de imagem e, se a medição pedir, para o
   planejador: uma segunda instância do motor em WASM, recebendo bytes +
   opções e devolvendo o resultado. É uma segunda saída de build (motor sem UI).
3. **Tamanho e carga:** `wasm-opt`, compressão brotli, cache com hash no nome,
   avaliar `opt-level` por crate (o motor precisa de velocidade; a UI aceita
   tamanho).
4. Opcional: PWA (service worker) para funcionar offline depois da primeira
   visita.

**Aceite:** traçar uma imagem não congela a interface; tempo de carregamento
e tamanho do WASM registrados neste plano.

*Threads reais em WASM (SharedArrayBuffer) ficam fora:* exigem cabeçalhos
COOP/COEP que o GitHub Pages não envia, e toolchain nightly.

### Fase 5 — Skitch por Web Bluetooth (M/G)

Pode correr em paralelo com as Fases 3 e 4 depois que a Fase 2 fechar.

1. **Camada assíncrona (3.4)** — primeiro no desktop, validada contra o `Sim`
   e contra os testes existentes, antes de qualquer código web.
2. **Transporte web** (`platform::ble` em `web-sys`, que exige
   `--cfg=web_sys_unstable_apis`):
   - `navigator.bluetooth.requestDevice({ filters: [{ services: [SERVICE_UUID] }] })`
   - `gatt.connect()` → serviço → características de escrita e leitura
     (`protocol::{SERVICE_UUID, WRITE_CHAR_UUID, READ_CHAR_UUID}`)
   - `request`: escreve com resposta, espera 50 ms, lê — como `Skitch::raw`
   - `send`: escreve os blocos do upload, aguardando cada promise
   - evento `gattserverdisconnected` → `Link::Lost`
3. **Interface:**
   - "Conectar" precisa partir de um clique (o navegador exige gesto do
     usuário e mostra o seu próprio seletor de dispositivo);
   - a reconexão automática ao abrir o app, que o desktop faz, vira um botão
     "Reconectar" na web;
   - texto de ajuda de pareamento adaptado; aviso em navegador sem Web
     Bluetooth (Firefox, Safari) apontando para Chrome/Edge.
4. **Simulador** disponível na web, para testar todo o fluxo sem a máquina.

**Aceite:** com o simulador, na web: conectar, enviar um desenho, iniciar,
pausar, simular quebra de linha, concluir. Com a máquina real: mesmo roteiro,
feito por quem tem a PP1.

**Incertezas próprias desta fase** (todas *a verificar* com a máquina real):

- Pareamento: a PP1 aceita a conexão mas responde vazio se não estiver
  pareada. No desktop o sistema operacional pareia ao conectar; no navegador
  isso depende do sistema e pode exigir parear antes nas configurações
  (sobretudo no Windows).
- Blocos de 500 bytes: o limite do Web Bluetooth por escrita é 512 bytes, então
  cabe; o ritmo de envio sem resposta pode precisar de ajuste.
- Gesto do usuário: o egui trata o clique no frame seguinte, não dentro do
  evento. A ativação do Chrome dura alguns segundos, então deve funcionar; se
  não, o botão "Conectar" vira um elemento HTML sobre o canvas.

#### Andamento da Fase 5 (2026-11-19)

**Mudança de rumo:** a seção 3.4 previa tornar a camada de máquina
assíncrona nos dois mundos. Ficou melhor que isso: o código é escrito uma
vez e compilado de dois jeitos, com a macro `maybe_async` — **síncrono no
desktop, exatamente como era** (a macro remove `async` e `.await`), e
assíncrono em WASM. O desktop não ganhou runtime, os testes não mudaram, e o
risco R6 (regressão no desktop) ficou restrito a uma revisão de diff.

Feito:

- [x] `src/machine/session.rs`: `Transport` e `Session` sob `maybe_async`;
      `pause()` no lugar de `thread::sleep` (a thread dorme no desktop; um
      temporizador na web); `MaybeSend` (`Send` só onde há threads).
- [x] `src/machine/sim.rs`, `src/machine/skitch.rs`: as implementações de
      `Transport` anotadas. O simulador roda igual nos dois mundos.
- [x] `src/studio/machine.rs`: o worker é o mesmo código; no desktop continua
      numa thread, na web é uma tarefa na página (`spawn_local`). Onde ele
      bloqueava esperando um comando, na web olha a fila a cada 30 ms.
- [x] `web/src/ble.js` + `platform::ble_link`: o transporte Web Bluetooth
      (`requestDevice` filtrando pelo serviço da PP1, escrever com resposta,
      esperar 50 ms, ler — como `skitch.rs`).
- [x] O build web liga a feature `ble`: a janela da máquina tenta o
      Bluetooth de verdade, e o simulador fica no menu "⋯ › Connect to a
      Simulated Machine".
- [x] Seletor fechado, ou sem clique recente (a reconexão automática ao
      abrir o app), vira "No Skitch found" com "Try Again".

Conferido no navegador, com a máquina simulada: conectar → "Needs homing" →
"Ready" → Send Design → "Trial run" → "Trial run done" → Start → "Sewing 12%,
407 / 3.326 stitches", com o progresso desenhado na janela e na área de
trabalho. A chamada ao Bluetooth do navegador foi conferida até o seletor de
dispositivos (simulado no teste, para não abrir o diálogo real).

Testes do desktop: iguais à linha de base. Observação: o teste
`machine::sim::tests::the_simulated_machine_goes_through_a_whole_job` falhou
uma vez ao rodar em paralelo com `--features gui,ble` e passou em seis
repetições isoladas — é baseado em tempo. Não sei se já oscilava antes desta
mudança; vale observar.

**Falta na Fase 5:**

- [ ] **Teste com uma PP1 de verdade.** Nada do transporte Web Bluetooth
      depois do seletor foi exercitado contra a máquina: pareamento, leitura
      e escrita das características, o envio em blocos de 500 bytes, queda
      de conexão.
- [ ] Evento `gattserverdisconnected`: hoje a queda é percebida na próxima
      pergunta à máquina (até 2 s depois), não na hora.
- [ ] Textos de ajuda da janela falam do desktop ("Pair the Skitch with this
      computer", o atalho para as configurações de Bluetooth do Mac): adaptar.
- [ ] `settings_log` (registro dos bytes de configuração da máquina) usa
      `OpenOptions` com append, que não existe nos arquivos em memória: na
      web não grava nada.
- [ ] O chip do MCP na barra de status ainda diz "started with --no-mcp"
      (Fase 6).

### Fase 6 — Relay MCP (M)

Objetivo: um agente (Claude Code, Codex, Claude Desktop) edita o desenho
aberto na aba, com as mesmas 33 ferramentas de `tools.rs`.

```
cliente MCP ──HTTP 127.0.0.1:7878──► zigzag-mcp-relay ──WebSocket 127.0.0.1:7879──► aba (studio WASM)
     (ou stdio via zigzag-mcp)            (nativo)                                  handle_rpc → tools.rs
```

1. **Relay** (`web/relay/`, binário nativo): atende MCP em HTTP na mesma porta
   e formato do studio desktop — as configurações de cliente e a ponte
   `zigzag-mcp` existentes funcionam sem mudança. Repassa cada mensagem
   JSON-RPC pela WebSocket e devolve a resposta com o mesmo `id`.
2. **Aba:** `web_sys::WebSocket`; cada mensagem entra em `mcp::handle_rpc`, que
   enfileira a chamada para o frame da UI, como o servidor HTTP faz hoje.
3. **Arquivos:** `open_file`, `save`, `export` e `trace_image` recebem caminhos
   do disco do agente, que a aba não alcança. O relay, que roda local, faz a
   leitura e a escrita: troca o caminho por bytes na ida e grava os bytes na
   volta. Para o agente, as ferramentas se comportam como no desktop.
4. **Segurança:**
   - escutar só em 127.0.0.1;
   - validar o cabeçalho `Origin` da WebSocket contra uma lista (o endereço do
     GitHub Pages e `localhost`);
   - código de pareamento: o relay imprime um código, o usuário digita na aba;
   - o proxy de arquivos limitado a uma pasta escolhida ao iniciar o relay.
5. **Interface:** o chip do MCP (`mcp_chip.rs`) mostra o estado da conexão com
   o relay e como instalá-lo. O painel de agente embutido (`agent.rs`) segue
   fora da web.
6. **Distribuição:** binários do relay nos releases do fork, ou
   `cargo install`.

**Aceite:** com a aba aberta e o relay rodando, o Claude Code lista as
ferramentas, adiciona um SVG, muda um objeto e exporta um PES para o disco.

**Incertezas:** uma página HTTPS abrindo `ws://127.0.0.1` funciona no Chrome,
mas versões recentes pedem permissão de acesso à rede local; Safari bloqueia.
*A verificar* no início da fase.

#### Andamento da Fase 6 (2026-11-19)

Feito, e conferido de ponta a ponta com um cliente MCP de verdade (`curl`
falando JSON-RPC com o relay, a aba aberta no navegador):

- [x] **`web/relay/`**: o programa `zigzag-mcp-relay` (crate próprio, nativo;
      `tiny_http` + `tungstenite`). Atende MCP em `http://127.0.0.1:7878/mcp`,
      o mesmo endereço do studio desktop, e a aba em `ws://127.0.0.1:7879`.
- [x] **Pareamento por link.** O relay imprime um link (`…/#relay=CÓDIGO`);
      abrir o ZZZ por ele pareia a aba. Sem o link a página nem tenta
      conectar — quem não usa agentes não vê nada (nem pedido de permissão de
      rede local). O código sai da barra de endereço e fica só na aba; colar
      um link novo numa aba já aberta repareia.
- [x] **A aba roda as ferramentas do próprio studio**: `platform::serve_agents`
      entrega cada mensagem a `mcp::handle_rpc`, com as mesmas 33 ferramentas
      de `tools.rs`, nas mesmas condições do desktop (espera enquanto há um
      diálogo ou um arrasto).
- [x] **Arquivos pelo relay.** Para `open_file` e `trace_image` o relay lê o
      arquivo e manda junto; para `save` e `export` a aba devolve o que foi
      escrito e o relay grava. Só dentro de uma pasta (`--dir`, por padrão
      onde o relay foi iniciado); caminho fora dela é recusado.
- [x] **Quem pode conectar.** MCP: como no desktop (só clientes locais, só
      JSON). Aba: só de uma origem permitida (páginas locais e o site do ZZZ;
      `--allow-origin` para outras) e que saiba o código.
- [x] **Chip do MCP na web** mostra o estado do relay e explica como ligar.

Roteiro conferido: `initialize` → `tools/list` (33) → `open_file groups.svg`
(lido do disco pelo relay) → `add_svg` (um círculo) → `export out.pes`
(gravado no disco, 20 KB, cabeçalho `#PES0001`) → `save saved.svg`. Sem aba:
erro dizendo qual link abrir. Origem de fora: 403. `../escape.pes`: recusado.

Duas coisas que só apareceram rodando:

- No navegador `Path::is_absolute()` é sempre falso (o `std` de
  `wasm32-unknown-unknown` não conhece caminho absoluto). As ferramentas
  exigiam caminho absoluto; lá passam a aceitar o que começa na raiz. E o
  relay traduz `C:.svg` para `/C:/a/b.svg` na ida, e de volta.
- `agent.rs`, `agent_log.rs` e `crash.rs` usavam o relógio do `std`, que
  derruba a página: trocados por `web_time` (a tela de erro da Fase 2
  mostrou a causa na hora).

Em `src/` nesta fase: `app.rs` (`handle_mcp` chama `platform::serve_agents`),
`mcp_chip.rs` (estado e texto na web), `tools.rs` (`full_path`), e o relógio
dos três módulos acima.

**Falta na Fase 6:**

- [ ] Testar com um cliente MCP completo (Claude Code, Claude Desktop pela
      ponte `zigzag-mcp`), e com o site publicado em HTTPS: a aba fala com
      `ws://127.0.0.1` a partir de uma página `https://`, o que o Chrome
      permite para o próprio computador mas pode pedir permissão de rede
      local nas versões recentes.
- [ ] Distribuição do relay: binários nos releases do fork, ou
      `cargo install --path web/relay`. Hoje só compila do fonte.
- [ ] Arquivos auxiliares que as ferramentas escrevem ao lado
      (`export` com `objects: true` grava `X.objects.json`): voltam pelo
      relay como qualquer escrita, mas não foi testado.
- [ ] Um documento aberto por um agente fica na aba com o caminho do
      computador do agente; depois de recarregar a página esse arquivo não
      está mais na memória.
- [ ] Aba em segundo plano: o navegador desacelera a página e as chamadas
      demoram. O relay avisa no erro de tempo esgotado; não há correção.

#### Revisão geral (2026-11-19)

Duas frentes: um revisor independente leu `git diff main...web` inteiro, e a
interface foi revista tela a tela no navegador, em tamanho real.

**Desktop:** nenhuma regressão encontrada no `src/`. A troca `std::fs` →
`vfs`, o `maybe_async`, o banco de fontes e os menus se comportam como
antes. A única mudança de comportamento (o worker da máquina avisa se a
thread não sobe) é uma melhoria.

**Textos que não faziam sentido na web, adaptados** (só no navegador; o
desktop segue igual):

- "About": "ZZZ", com o link do fork e o crédito ao ZigZag com o link do
  projeto original.
- Help: "ZZZ on GitHub" e "Report an Issue…" vão para o fork.
- Settings: sem a seção "Agent" (agente embutido e servidor MCP do
  desktop) e sem o grupo "Updates".
- Window: o painel "Agent" saiu da lista.
- Janela da Skitch: as instruções falam de escolher a máquina na lista que o
  navegador mostra. (As do Mac já não apareciam: são só para macOS.)
- Dica do botão "+" das abas sem o atalho ⌘N (que o navegador guarda); texto
  de "máquina que este computador não tem"; descrição de `list_fonts` para
  agentes.

**Achados do revisor, corrigidos:**

| # | Achado | Correção |
|---|---|---|
| 1 | "Check Now" (Settings › Updates) derrubava a página | Grupo Updates não existe na web |
| 2 | Dois arquivos com o mesmo nome se sobrescreviam (na memória, no IndexedDB e no disco) | `files.js::claim_name`: o mesmo arquivo mantém o nome; outro recebe "logo (2).svg" |
| 3 | Agente salvando o documento da pessoa gravava na pasta do relay; pessoa salvando o documento do agente abria um download | `platform::send_on`: o destino depende de quem é o arquivo (studio, pessoa, agente), não de quem salvou |
| 4 | Código de pareamento de 6 dígitos, adivinhável; a aba podia gravar qualquer arquivo na pasta | Código de 128 bits; o relay só grava os arquivos que a chamada pediu |
| 5 | Link simbólico como último trecho do caminho escapava da pasta | `within` confere para onde o link leva |
| 6 | Duas abas pareadas se derrubavam em loop | A aba substituída é avisada e não reconecta |
| 7 | Chamadas de agente com a aba ocupada ficavam paradas, e rodavam depois do prazo | Repinta até poder rodar; chamadas com mais de 170 s são descartadas |
| 8 | Duas gravações rápidas no mesmo arquivo podiam terminar fora de ordem | Gravações em fila |
| 10 | Mensagens acima de 16 MB derrubariam a aba do relay | Limite de 512 MB |

Conferido no navegador depois das correções: About, Help, Window, Settings,
janela da Skitch, dois `logo.svg` diferentes, e o relay de ponta a ponta
(abrir, exportar com `objects: true`, salvar). Uma regressão minha apareceu
nesse teste e foi corrigida: o `relay.js` só aceitava códigos numéricos.

**Achados registrados, não corrigidos:**

- Arquivos escolhidos ou soltos ficam no IndexedDB para sempre e são todos
  carregados no início (achado 9). Precisa de uma limpeza dos que não estão
  em nenhuma aba.
- Fechar o seletor de "Salvar" sem escolher deixa o documento marcado como
  salvo (parte do achado 8): o resultado da gravação não volta ao studio.
- Os registros de configurações da Skitch e de chamadas de agente não são
  gravados na web (achado 11): usam `OpenOptions` com append.
- A barra de menus espremida ("FileEditObject…") **vem do ZigZag original**:
  a mesma coisa aparece no desktop do Windows. Uma fonte melhor (Inter)
  ajudaria; com ela embutida, fica igual ao desktop no Windows.
- No desktop Windows, o painel do agente diz "isn't installed on this Mac":
  bug do original, fora do escopo do ZZZ.
- A tela de carregamento é sempre escura, mesmo com o tema claro.

#### Verificação final (2026-11-19)

Testes: desktop (504 passam; a mesma falha antiga do teste do agente, que
usa `#!/bin/sh` e não roda no Windows), módulos do web, motor em release no
desktop e em WASM (9 de 9, com o `sample1.pes`), relay (3 de 3), e
`cargo check --features gui,ble`.

No navegador, de ponta a ponta, com os seletores de arquivo do navegador
simulados (são diálogos do sistema) e cada arquivo gravado conferido:

| Recurso | Resultado |
|---|---|
| Carregar a página | Sem erros; 29 arquivos da biblioteca de fontes buscados |
| File › Open (seletor) | `groups.svg` aberto |
| Selecionar, mover, desfazer, refazer | "Moved 5 objects"; Ctrl+Z volta a 0 mm; Ctrl+Shift+Z refaz |
| Trocar tipo de ponto | Satin; o aviso de largura do próprio studio aparece |
| Ctrl+S | Grava `groups.svg` de volta no mesmo arquivo; o "•" some |
| File › Export › PES / DST | `groups.pes` (`#PES0001`, 17 KB), `groups.dst` (`LA:groups`, 8 KB) |
| Soltar PES | `sample1.pes`: 11 cores, 11.807 pontos |
| Soltar PNG (traçar) | Janela Trace Image, 2 cores; abre como documento |
| Ferramenta retângulo | "Drew a rectangle (11.9 × 8.9 mm)", preenchido |
| Recarregar a página | Volta com as abas e o retângulo não salvo |
| Skitch simulada | Conectar → Ready → Send → trial run → Sewing 10% |
| Letras e fontes | Conferido na rodada anterior (Poppins, Pacifico) |
| Relay MCP | Conferido de ponta a ponta depois das correções da revisão |

Achado e corrigido nesta rodada:

- A barra de menus espremida (vinha do original, também no desktop do
  Windows): os títulos ganharam espaço (`commands::menu_bar`). Conferido no
  navegador e no desktop.
- O teste da biblioteca de fontes falhava às vezes no desktop: outro teste
  adicionava uma fonte ao mesmo tempo. Agora lê as famílias dos próprios
  arquivos.

Observado, não alterado (comportamento do original): um arquivo de máquina
(PES) aberto e não editado não volta ao recarregar, porque abre como cópia
sem arquivo por trás; a dica da ferramenta de forma fala em "Option", a
tecla do Mac (Alt no Windows), também no desktop.

### Fase 7 — Publicação (P)

Pode começar logo depois da Fase 2, para ter um endereço de teste cedo.

1. `.github/workflows/web.yml`: instala o toolchain, `trunk build --release
   --public-url /zigzag/`, `wasm-opt`, publica no GitHub Pages.
2. Roda em push na branch `web`; pull requests só fazem build e testes.
3. Página com versão e commit visíveis ("Sobre"), e link para o upstream.
4. `THIRD-PARTY-NOTICES.md` atualizado com as dependências web e as fontes
   embutidas; `web/README.md` com como compilar e rodar.

**Aceite:** push na branch publica; o endereço público abre e passa no
roteiro de aceite da Fase 2.

#### Andamento da Fase 7 (2026-11-19)

Pronto para publicar; falta só o que é da conta do GitHub (push e
configurações), que fica com o dono do repositório.

- [x] `.github/workflows/web.yml`: em push na branch `web` (e à mão):
      testa (módulos, motor nativo, relay), compila com Trunk usando o nome
      do repositório como prefixo dos caminhos, e publica no GitHub Pages.
      Pull requests para `web` testam e compilam, sem publicar. Os dois
      workflows do upstream só rodam em releases: o push da `web` não os
      dispara.
- [x] Dependências do navegador restritas a WebAssembly
      (`web/Cargo.toml`): o build nativo dos testes, no runner Linux, fica
      só com o motor, sem bibliotecas de janela ou do sistema. O teste que
      usava a fonte do egui passou a usar a Inter.
- [x] Conferido localmente: build com `--public-url /zigzag/`, servido sob
      `/zigzag/` com os tipos de arquivo do Pages; a página abre e busca
      as 29 fontes.
- [x] `web/README.md`: o que é, o que funciona, agentes, como compilar.
- [x] Remoto `fork` (`quilombodigital/zigzag`) adicionado ao clone: o
      `origin` aponta para o upstream (`simpsoka/zigzag`).

**Para publicar (dono do repositório):**

1. `git push fork web`
2. Settings › Pages › Build and deployment › Source: **GitHub Actions**.
3. Settings › Environments › **github-pages** › Deployment branches and
   tags: acrescentar `web` (por padrão só a branch padrão publica).
4. Actions › web › Run workflow (ou um novo push).

Site: `https://quilombodigital.github.io/zigzag/`.

**Não verificado:** o workflow em si (só roda no GitHub); a página em HTTPS
falando com o relay em `ws://127.0.0.1` (o Chrome pode pedir permissão de
rede local).

**Ficou para depois:** versão e commit visíveis na página; avisos de
terceiros (`THIRD-PARTY-NOTICES`) cobrindo as dependências do build web.

---

## 5. Ordem e dependências

```
Fase 0 ─► Fase 1 ─► Fase 2 ─┬─► Fase 3 ─► Fase 4
                            ├─► Fase 5 (Bluetooth)
                            ├─► Fase 7 (publicação, cedo)
                            └─► Fase 6 (relay) — depois da 3, pois usa o modelo de documentos
```

**Primeira versão pública** = Fases 0, 1, 2, 3, 5 e 7.
Fase 4 entra conforme a medição pedir; Fase 6 depois.

---

## 6. Diferenças planejadas em relação ao desktop

| Recurso | Web |
|---|---|
| Atualizador | Não existe; a página é sempre a última versão |
| Log de crash | Tela de erro com detalhes e "recarregar"; sessão recuperada |
| Painel de agente embutido | Fora |
| Servidor MCP | Via relay (Fase 6) |
| Menu nativo do macOS | Menus desenhados na janela |
| Fontes do sistema | Embutidas + enviadas + Local Font Access (opcional) |
| Reconexão automática à Skitch | Botão "Reconectar" |
| Salvar no mesmo arquivo | Só Chrome/Edge (File System Access); nos demais, download |
| Bluetooth | Só Chrome/Edge |
| Atalhos reservados pelo navegador | Alternativos |
| CLI `zigzag` | Não se aplica |

---

## 7. Riscos

| # | Risco | Impacto | Mitigação |
|---|---|---|---|
| R1 | Alguma dependência não compila em WASM | Bloqueia | Fase 0 existe para isso; replanejar ali |
| R2 | Mudanças em `src/` conflitam a cada release do upstream (ritmo alto: 0.1.0 → 0.1.5) | Custo recorrente | Concentrar diferenças em `platform/`; trocas mecânicas e pequenas; CI web em todo merge |
| R3 | Panic em WASM aborta o app; `design::caught` (que hoje salva o planejador de geometria ruim) deixa de capturar | Um desenho problemático derruba a aba | Recuperação de sessão + tela de erro; guardar "o último documento derrubou o app" como `session.rs` já faz; avaliar depois `panic=unwind` em WASM (nightly) |
| R4 | Planejador mais lento em WASM, uma thread só | Edição menos fluida em desenhos grandes | Medir na Fase 2; worker na Fase 4 |
| R5 | Web Bluetooth com a PP1 real: pareamento, ritmo de envio | Fase 5 atrasa | Simulador cobre o fluxo; teste com a máquina cedo |
| R6 | Camada de máquina assíncrona introduz regressão no desktop | Quebra o que funciona | Fazer primeiro no desktop, com os testes do `Sim` |
| R7 | WebGL2 se comporta diferente do OpenGL no shader de pontos | Visual errado | Caminho sem GPU já existe |
| R8 | Tamanho do WASM grande (motor + egui + `image` + `resvg`) | Carga lenta | `wasm-opt`, brotli, cache; medir na Fase 0 |
| R9 | IndexedDB apagado pelo navegador | Perda de trabalho não salvo | `storage.persist()`, incentivar salvar em arquivo |
| R10 | `#[path]` ficar frágil | Manutenção | Alternativa `zigzag-studio` como lib (3.1) |
| R11 | Restrições do navegador ao relay em `localhost` | Fase 6 muda de desenho | Verificar no início da fase |

---

## 8. Testes

| Camada | Como |
|---|---|
| Regressão do desktop | `cargo test` e `cargo test --features gui` em todo commit — a suíte `egui_kittest` existente é a rede de segurança das mudanças em `src/` |
| Compila em WASM | `cargo check --target wasm32-unknown-unknown` do motor e do `zigzag-web` no CI |
| Motor em WASM | `wasm-bindgen-test`: abrir, planejar e exportar os exemplos |
| Plataforma web | `wasm-bindgen-test` em navegador headless: armazenamento, documentos |
| Ponta a ponta | Playwright: carregar, abrir exemplo, exportar, comparar com o CLI nativo |
| Bluetooth | Simulador na web; roteiro manual com a máquina |
| Relay | Teste de integração: cliente MCP falso ↔ relay ↔ aba headless |
| Lista de módulos | Teste que compara `src/studio/main.rs` com `web/src/lib.rs` |

---

## 9. Convivência com o upstream

- `main` acompanha `simpsoka/zigzag` sem alterações; o trabalho vive na branch
  `web`.
- A cada release do upstream: merge em `web`, rodar o CI web, classificar
  módulos novos do studio (entram na web ou não).
- Regra para mudanças em `src/`: a menor possível, atrás de `platform::` ou de
  `cfg(target_arch = "wasm32")`, sem mudar comportamento no desktop. Exceção
  consciente: a camada de máquina assíncrona (Fase 5).
- Manter neste arquivo a lista dos arquivos de `src/` tocados, para saber onde
  esperar conflito.

---

## 10. Ferramentas

| Ferramenta | Para quê |
|---|---|
| `rustup`, Rust 1.95, target `wasm32-unknown-unknown` | Compilar |
| `trunk` | Build e servidor de desenvolvimento |
| `wasm-bindgen-cli` | Bindings JS (versão casada com a do crate) |
| `binaryen` (`wasm-opt`) | Reduzir o WASM |
| Node + Playwright | Testes de navegador |
| Chrome ou Edge | Navegador de referência (WebGL2, Web Bluetooth, File System Access) |

---

## 11. Perguntas em aberto

1. **Você tem uma Brother Skitch (PP1) para testar?** Sem ela, a Fase 5 só é
   validada contra o simulador.
2. **Hospedagem:** GitHub Pages em `quilombodigital.github.io/zigzag/`, ou
   domínio próprio?
3. **Fontes embutidas:** quais? Sugestão: três ou quatro famílias OFL que
   bordam bem (uma sem serifa, uma com serifa, uma script, uma em bloco).
4. **Branch:** `web` a partir de `main`, como proposto?
5. **Ícone do zzz:** reaproveita o ícone do ZigZag ou ganha um próprio?

---

## Registro de mudanças

| Data | Mudança |
|---|---|
| 2026-11-18 | Primeira versão do plano, a partir da leitura do código (upstream 0.1.5). Nenhum build feito. |
| 2026-11-19 | Reorganização em workspace: motor na raiz, `crates/studio` (biblioteca + app do desktop), `crates/web`, `crates/relay`, `docs/`. Os `#[path]` deram lugar à dependência normal do studio. |
| 2026-11-19 | Fase 7: workflow do GitHub Pages, README do web, dependências do navegador só em WASM. Pronto para o push. |
| 2026-11-19 | Verificação final: todos os testes e um roteiro completo no navegador; barra de menus espaçada (web e desktop). |
| 2026-11-19 | Biblioteca de 27 fontes para bordar, carregada em segundo plano; lista de fontes do painel passa a se atualizar. |
| 2026-11-19 | Inter 4.1 embutida (interface e letras). |
| 2026-11-19 | Revisão geral: nenhuma regressão no desktop; textos do desktop adaptados na web; 9 achados do revisor corrigidos (inclusive um travamento e dois de segurança no relay), 4 registrados. |
| 2026-11-19 | Fase 6: relay MCP (`web/relay/`), pareamento por link, ferramentas do studio rodando na aba, arquivos lidos e gravados pelo relay. Conferido de ponta a ponta. |
| 2026-11-19 | Fase 5: camada de máquina compilada de dois jeitos com `maybe_async` (síncrona no desktop, assíncrona na web) em vez de assíncrona nos dois; transporte Web Bluetooth; fluxo completo conferido com o simulador no navegador. |
| 2026-11-19 | Fase 3: persistência em IndexedDB, abrir/salvar pela File System Access API, "Add Fonts…". Fontes embutidas aguardam decisão. |
| 2026-11-19 | Fase 2, segunda rodada: o que é só do desktop sai da interface, página com carregamento/aviso/erro, texto com fontes mínimas, tempos de planejamento medidos (R4 não se confirmou). |
| 2026-11-19 | Fase 2, primeiro marco: o studio compila e abre no navegador; abre SVG e PES, salva por download. Duas mudanças de rumo: nenhum módulo excluído, e `zigzag::vfs` no lugar do `platform::store`. |
| 2026-11-18 | Fase 1 concluída: motor abre por bytes, fontes recarregáveis, 8 testes passam em WASM com saída idêntica ao desktop em 6 de 7 desenhos. Ver "Resultado da Fase 1". |
| 2026-11-18 | Fase 0 concluída: motor compila e roda em WASM, hello egui abre no navegador, inventário de 59 erros do studio. Ver "Resultado da Fase 0". |
| 2026-11-18 | Nome passa a "ZZZ" em maiúsculas; `sample1.pes` adotado como arquivo de teste de importação (D9). |
| 2026-11-18 | Decisões D6 (inglês), D7 (só Chrome/Edge) e D8 (nome "zzz"). D7 simplifica as Fases 2, 3 e 5: sem caminhos alternativos para outros navegadores. |
