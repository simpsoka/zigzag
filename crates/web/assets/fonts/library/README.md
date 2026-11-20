# Fontes para bordar

Fontes que o ZZZ traz para as letras bordadas. O `library.json` é o índice que a página lê; a página busca as fontes em segundo plano depois que o studio abre (`crates/studio/src/platform/files.js`, `font_library`).

Origem: repositório oficial do Google Fonts, <https://github.com/google/fonts>, ramo `main`, baixadas em 2026-11-19. Cada pasta traz a licença da família (`OFL.txt` ou `LICENSE.txt`). Arquivos com `-Variable` no nome são as fontes variáveis do repositório (`Família[wght].ttf`), renomeadas; o bordado usa a instância padrão (Regular).

Para acrescentar uma família: coloque a pasta aqui, com a licença, e acrescente a fonte ao `library.json` e ao teste `every_font_in_the_library_embroiders` em `crates/web/tests/engine.rs`.

## Sem serifa

| Família | Arquivos | Licença |
|---|---|---|
| anton | Anton-Regular.ttf | SIL OFL 1.1 |
| archivoblack | ArchivoBlack-Regular.ttf | SIL OFL 1.1 |
| bebasneue | BebasNeue-Regular.ttf | SIL OFL 1.1 |
| fredoka | Fredoka-Variable.ttf | SIL OFL 1.1 |
| montserrat | Montserrat-Variable.ttf | SIL OFL 1.1 |
| nunito | Nunito-Variable.ttf | SIL OFL 1.1 |
| oswald | Oswald-Variable.ttf | SIL OFL 1.1 |
| poppins | Poppins-Bold.ttf, Poppins-Regular.ttf | SIL OFL 1.1 |
| righteous | Righteous-Regular.ttf | SIL OFL 1.1 |

## Com serifa

| Família | Arquivos | Licença |
|---|---|---|
| abrilfatface | AbrilFatface-Regular.ttf | SIL OFL 1.1 |
| alfaslabone | AlfaSlabOne-Regular.ttf | SIL OFL 1.1 |
| cinzel | Cinzel-Variable.ttf | SIL OFL 1.1 |
| lora | Lora-Variable.ttf | SIL OFL 1.1 |
| playfairdisplay | PlayfairDisplay-Variable.ttf | SIL OFL 1.1 |

## Cursivas e manuscritas

| Família | Arquivos | Licença |
|---|---|---|
| caveat | Caveat-Variable.ttf | SIL OFL 1.1 |
| courgette | Courgette-Regular.ttf | SIL OFL 1.1 |
| dancingscript | DancingScript-Variable.ttf | SIL OFL 1.1 |
| greatvibes | GreatVibes-Regular.ttf | SIL OFL 1.1 |
| kaushanscript | KaushanScript-Regular.ttf | SIL OFL 1.1 |
| lobster | Lobster-Regular.ttf | SIL OFL 1.1 |
| pacifico | Pacifico-Regular.ttf | SIL OFL 1.1 |
| permanentmarker | PermanentMarker-Regular.ttf | Apache 2.0 |
| satisfy | Satisfy-Regular.ttf | Apache 2.0 |

## Bloco, colegial e display

| Família | Arquivos | Licença |
|---|---|---|
| blackopsone | BlackOpsOne-Regular.ttf | SIL OFL 1.1 |
| bungee | Bungee-Regular.ttf | SIL OFL 1.1 |
| graduate | Graduate-Regular.ttf | SIL OFL 1.1 |
| staatliches | Staatliches-Regular.ttf | SIL OFL 1.1 |
