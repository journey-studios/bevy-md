# Handoff — bevy_md

Atualizado em 2026-11-09. Repositório: https://github.com/journey-studios/bevy-md
Branch de entrega: `feat/initial-bevy-markdown`.

## Objetivo

Biblioteca Rust de Markdown para Bevy: core independente da engine, renderização
incremental e virtualizada, streaming e editor de texto rico. Esta frente é
separada do Tessel Desktop; integração ao aplicativo ainda não foi comprovada.

## O que foi feito

- Três crates: `bevy_md_core`, `bevy_md` e `bevy_md_editor`.
- Parse incremental, árvore de somas, source map e índice de definições de links
  e notas de rodapé.
- Splices disjuntos e composição de edições: mudanças distantes preservam os
  blocos intermediários, inclusive entidades e alturas medidas no adaptador Bevy.
- Virtualização, estimativas/medidas de altura, âncora de scroll e pool de entidades.
- Parse assíncrono revisionado, fonte compartilhada e índice de alturas preparado
  no worker. Entrada recebida durante parse fica em fila.
- Editor no bloco focado, caret, gutter visual e undo/redo por deltas.
- Edição básica de texto (2026-11-09), cada frente em seu módulo de
  `crates/bevy_md_editor/src` e seu teste em `tests/`: `selection` (Shift, Ctrl+A,
  Esc, digitar sobre a seleção), `navigation` (↑/↓, entre blocos, páginas),
  `pointer` (clique duplo/triplo, arrastar), `clipboard` (copiar, recortar,
  colar sobre a seleção), `blocks` (Enter, unir blocos) e `words` (Ctrl+setas,
  Ctrl+Backspace/Delete).
- Ponteiro e caret visíveis (2026-11-09): clique em qualquer ponto da linha (à
  direita/esquerda, entre blocos ou abaixo do fim) leva o caret ao ponto mais
  próximo (`Hits::locate_click` em `pointer.rs`); o cursor do mouse vira I-beam
  sobre o documento e mão sobre links (`hover.rs`, só pelo ícone da janela;
  `bevy_feathers` ou `OverrideCursor` podem sobrescrever); `gap.rs` desenha o
  caret piscando em linha em branco entre blocos (`GapCaret`), pois o Markdown
  não tem parágrafo vazio e o Bevy não desenha caret ali. Clicar em link ainda
  não abre nada.
- Testes de propriedades, orçamentos de custo e harness de janela nativa.

O histórico detalhado, medições e ressalvas estão em
[fase-2-performance.md](plans/fase-2-performance.md). O roadmap completo está em
[PLAN.md](../PLAN.md); não considerar a fase 2 concluída.

## O que falta

1. Preservar alterações grandes distantes que pousam juntas no parse assíncrono;
   ainda podem virar um intervalo que cobre o meio. Também há fallback após 256
   trechos e em casos conservadores de definições duplicadas/brackets abertos.
2. Medir e reduzir o custo da primeira edição: a fonte pode realocar cerca de
   10 MB em documentos de 120 mil blocos. Orçamentos aquecidos não provam esse caso.
3. Fechar M1: tecla abaixo de 20 µs em 1 MB e streaming dentro de +10% do baseline
   antigo seguem sem prova comparável. Não inventar um baseline M0 recuperado.
4. Fechar M6: abrir 5 MB frio sem frame acima de 16 ms ainda falha em algumas
   rodadas. Manter todas as cinco rodadas na aceitação, sem escolher a melhor.
5. Provar scroll estável a 120 fps. O gate mede cadência aproximada de render;
   não mede timestamps de apresentação nem GPU. Não confundir com FPS apresentado.
6. Editor completo e ecossistema: IME com composição, clipboard do sistema, UX de
   blocos (fase 4) e integração ao Tessel são frentes posteriores. Lacunas da
   edição básica: rolar até o caret, rolagem ao arrastar, destaque da seleção
   fora do bloco do caret, blocos sem source map (código, HTML, tabela) e Enter
   dentro de link ou estilos mistos.
7. Windows: `.gitattributes` fixa LF (os testes comparam bytes). As sondas de CPU
   (`rustix`) só existem em Unix e leem zero no Windows. Falham no Windows por
   orçamento de alocações calibrado em outra plataforma: `bevy_md` `budgets`
   (`idle_allocations_*`, medido 88 contra limite 70) e `far_edits`
   (`streaming_a_definition_reaches_the_top_without_touching_the_rest`).
   Recalibrar ou separar o orçamento por plataforma antes de usar como gate.

## Arquivos e evidência

- `crates/bevy_md_core/src/{document,definitions,positions,splices,sum_tree}.rs`:
  semântica e índices independentes de Bevy.
- `crates/bevy_md/src/{background,heights,virtualize,gutter}.rs`: adaptador UI.
- `crates/bevy_md_editor/src/`: entrada, caret e histórico.
- `crates/bevy_md/tests/{background,paste,far_edits,virtualization}.rs` e
  `crates/bevy_md_core/tests/definitions.rs`: regressões importantes.
- `perf-budgets.json`: limites e medições; alguns testes relativos usam folga e
  aquecimento. Conferir metodologia antes de alegar custo invariável.
- `scripts/native-gate.sh` e `crates/bevy_md_editor/examples/native_gate.rs`:
  janela real, configuração e amostragem.
- `.github/workflows/ci.yml`: conferir cobertura dos três crates; CI remoto não é
  equivalente à validação local.

Capturas e traces locais em `docs/evidence/` e `docs/screenshots/` não fazem parte
da publicação inicial. Referências a esses caminhos nos planos são evidência
local histórica. O comparativo GPUI/Bevy ficou no checkout separado do Tessel
Desktop em `docs/evidence/perf-compare-gpui-bevy/REPORT.md`, não nesta biblioteca.
Os apps foram executados sequencialmente; fontes, conversão Markdown e fronteiras
de medição diferem. Não é um benchmark isolado dos frameworks.

## Comandos para rodar

Na raiz do repositório, com a toolchain de `rust-toolchain.toml`:

```sh
cargo run --release -p bevy_md --example streaming
cargo run --release -p bevy_md --example streaming -- --blocks 100000
cargo run --release -p bevy_md_editor --example spike

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast -- --test-threads=1
BEVY_MD_SEEDS=3000 cargo test -p bevy_md_core --release --test incremental
BEVY_MD_SEEDS=3000 cargo test -p bevy_md_core --release --test definitions
BEVY_MD_SEEDS=3000 cargo test -p bevy_md_editor --test history
cargo bench -p bevy_md_core

# macOS, display desbloqueado; executa uma janela por vez
ROUNDS=3 BLOCKS="200 2000 100000" scripts/native-gate.sh
```

Rodar benchmarks sequencialmente, sem builds simultâneos. Registrar carga,
perfil, viewport, fontes e vsync; preservar falhas e máximos.
Neste ambiente `cargo` passa pelo `mbx`: usar o target padrão, nunca definir
`CARGO_TARGET_DIR`, `--target-dir` ou `build.target-dir`. Não criar caches/builds
grandes em `/tmp`. Preservar trabalhos locais e não limpar targets manualmente.

## Validação conhecida

A validação desta entrega executou `cargo fmt --check` e toda a suíte de
`bevy_md_core` com sucesso. A revisão independente anterior validou quatro propriedades de
composição de splices, dois testes de referências relacionados e dez testes de
edições distantes. O implementador relatou workspace, clippy e propriedades com
3000 seeds verdes; são camadas de evidência distintas. Testes de tempo podem
falhar legitimamente, sobretudo na abertura fria de 5 MB. Publicar código não
constitui aprovação de performance ou publicação no crates.io.

## Skills sugeridas

- `diagnosing-bugs`: investigar picos por estágio com hipótese e evidência.
- `commit`: antes de criar novos commits.
- `tessel-desktop-architecture`: apenas ao integrar no produto Tessel Desktop;
  localizar o app Bevy existente e não confundir com o editor de vídeo homônimo.
