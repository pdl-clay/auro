# Auro

[![Stars](https://img.shields.io/github/stars/pdl-clay/auro?style=flat-square&label=stars)](https://github.com/pdl-clay/auro/stargazers)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square)](#licença)
[![Status](https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square)](#status)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square)](#status)

[English](README.md) · **Português**

**O modelo de app do Tauri. O renderer do GPUI. Zero webview.**

Auro é um framework de aplicação para apps desktop em Rust. Ele pega as partes do Tauri que
dão trabalho para reconstruir — configuração, capabilities, plugins, bundler, updater assinado,
dev server, single-instance, multi-window — e troca o renderer: em vez de uma webview, a UI é o
GPUI, o toolkit de UI em Rust acelerado por GPU do Zed.

O frontend é Rust. Não existe HTML, nem engine de CSS, nem JavaScript no runtime do app.

---

## Status

**Pré-alpha. Projetando em público.** O workspace já existe, as fases 0 e 1 estão concluídas e a
fase 2 está em andamento. `0.1.0-alpha.9` é o alpha atual publicado no crates.io; ainda não há
release estável nem CLI estável. Instale o CLI alpha com
`cargo install auro-cli --version 0.1.0-alpha.9`, ou compile e experimente o framework a partir de
um checkout (veja [CONTRIBUTING.md](CONTRIBUTING.md)). As fatias da fase 2 no repositório cobrem empacotamento,
updates assinados com caminhos de aplicação por plataforma e serviços de plataforma; submissão e
verificação local de notarização no macOS são ferramentas de release, não um fluxo de usuário
testado. Hot reload está medido, não GA, e só para a fronteira de dados do scaffold opt-in de
quatro crates: no Linux, edições em `data/src` (a fronteira `view_data() -> HotData`) ficam abaixo
da meta de 500 ms; no Windows e em um runner macOS compartilhado, acima. Edições em `ui`/`Render`,
layout, `types`, dependências ou configuração exigem rebuild do host e restart do processo (com
restauração de snapshot quando compatível); não são reload de view no mesmo processo.

| Fase | Codinome | Estado |
|---|---|---|
| 0 | Foundations | **concluída** |
| 1 | Hello, native | **concluída** |
| 2 | Ship it | **em andamento** |
| 3 | Extensible | planejada |
| 4 | Ecosystem | planejada |
| 5 | 1.0 | planejada |

Detalhe completo no [ROADMAP.md](ROADMAP.md) (em inglês). A arquitetura e o raciocínio por trás
dela estão no [livro de documentação](https://pdl-clay.github.io/auro/); o suporte de plataformas
é [publicado como matriz](docs/src/reference/platform-support.md) desde o primeiro dia.

## Por quê

Electron e Tauri embarcam um navegador inteiro para desenhar um botão.

| | Electron | Tauri | Auro |
|---|---|---|---|
| Modelo de app | mínimo | **excelente** | o do Tauri |
| Renderer | Chromium | webview do sistema | **GPU, nativo** |
| Linguagem do frontend | JS/TS | JS/TS | **Rust** |
| RAM idle (hello world, medido) | 291 MB | 315 MB | **99 MB** |
| Cold start (conteúdo pronto, medido) | 421 ms | 417 ms | **190 ms** |
| Payload (não comprimido, medido) | 274 MB | **6,7 MB** | 14,9 MB |
| Texto/IME/seleção | do navegador | do navegador | herdado de um editor |

Medido em Linux/Wayland (niri), 2026-11-19: uma janela 480x320 com um label, PSS da árvore de
processos após 5 s idle, spawn até conteúdo pronto (não é frame pintado), payload não comprimido.
O payload do Electron inclui o Chromium; Auro e Tauri usam o webview/GPU do sistema. Método e
amostras cruas: [performance budget](docs/src/reference/performance-budget.md) e
[`tools/comparison/baseline.json`](tools/comparison/baseline.json).

O Tauri acertou a parte chata e difícil: capabilities, sistema de plugins, empacotamento em sete
formatos, updater assinado. O que ele erra, para quem quer *nativo*, é que o app continua sendo
uma página web.

Auro mantém o casco e troca o motor.

## Como vai ser

```rust
use auro::prelude::*;

#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> {
    tokio::fs::write(&path, body).await?;
    Ok(Saved { path, bytes: body.len() })
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Button::new("save", "Save").on_click(cx.listener(
                |this, _, _window, cx| {
                    let doc = this.document.clone();
                    cx.spawn(async move |this, cx| {
                        let saved = api::save_document(doc.path, doc.body).await?;
                        this.update(cx, |this, cx| {
                            this.status = format!("Saved {} bytes", saved.bytes);
                            cx.notify();
                        });
                        Ok::<(), AuroError>(())
                    })
                    .detach();
                },
            )))
            .child(self.status.clone())
    }
}

fn main() {
    auro::App::new()
        .root::<Editor>("main")
        .run();
}
```

Sem `invoke("save_document", {...})`. O comando é uma função Rust; o ponto de chamada é uma
chamada Rust. Erro de tipo é erro de compilação.

## As três decisões que definem o projeto

1. **IPC tipado, em tudo.** Os schemas saem das assinaturas Rust. Não existe ponte
   stringly-typed, porque não existe JavaScript para atravessar.
2. **Hot reload ou nada.** O tempo de compilação do Rust é o único motivo real para continuar na
   web. Meta: editar e ver em **menos de 500 ms**, com o estado do app preservado. Todo o projeto
   depende dessa aposta.
3. **Capabilities na fronteira de verdade.** Com frontend em Rust, a maior parte da checagem de
   permissão é em tempo de compilação. O que realmente precisa de cerca em runtime é **código de
   plugin** e **conteúdo não confiável**. É aí que mora o modelo de capabilities do Auro.

## O que o Auro não é

- **Não é wrapper de webview.** Webview só existe como escape hatch com escopo explícito para
  OAuth e docs embutidos; não é a fundação.
- **Não é toolkit de GUI.** Isso é o GPUI. Auro é framework de *aplicação*.
- **Não é biblioteca de componentes.** O Auro entrega a camada de aplicação; use o GPUI direto ou
  escolha um toolkit como `gpui-component`. `App::setup` e `App::root_with` são a costura; o
  framework não empacota widgets.
- **Não é fork do GPUI.** Acompanhamos upstream com uma camada fina de compatibilidade.
- **Não é "CSS para Rust".** Não vai existir linguagem de estilo. Se você quer CSS, use Tauri.

## Performance que estamos perseguindo

| Métrica | Meta no 1.0 | Medido (2026-11-18) |
|---|---|---|
| Cold start | < 80 ms | 154,9 ms mediana, requisição do primeiro frame (Linux) |
| RAM idle | < 40 MB | 105,3 MB (Linux) |
| Binário hello-world | < 6 MB (stripped) | 15,6 MB (Linux) |
| Editar e ver (só fronteira de dados) | < 500 ms | 368 ms mediana, p95 394 ms para edições em `data/src` (`view_data() -> HotData`), hot mode opt-in, N=12 (Linux); 813 ms mediana, p95 897 ms (Windows); 509,5-956,0 ms medianas, p95 até 2096 ms, três runs com seis warm-ups (runner macOS compartilhado) |
| Plataformas | macOS 12+, Windows 10+, Ubuntu 22.04+ (Wayland e X11) | Linux verificado em runtime local; macOS e Windows compilam e testam no CI |

São medições alpha, não uma baseline 1.0, e o gate de regressão no CI ainda não existe: o harness
existe e o método está no [budget de performance](docs/src/reference/performance-budget.md). O
resultado de editar e ver cobre só a fronteira de dados do caminho opt-in de hot reload com o
scaffold de quatro crates; hot reload está medido, não GA. Uma mudança em `ui`/`Render`, layout,
`types`, dependências ou configuração é código de host: `auro dev` recompila e reinicia o
processo, restaura o estado opt-in a partir do snapshot quando a versão é compatível e rotula o
resultado como `restarted (state restored)` ou `restarted (state reset: <reason>)`. Esse caminho
nunca é reportado como reload. O status por plataforma está na
[matriz de suporte](docs/src/reference/platform-support.md).

## Roadmap

Seis fases, ~42 semanas até o 1.0. Detalhe completo no [ROADMAP.md](ROADMAP.md) (em inglês).

| Fase | Codinome | Objetivo |
|---|---|---|
| 0 | Foundations | `auro new` até uma janela, em menos de dois minutos |
| 1 | Hello, native | App layer + IPC tipado + `auro dev`, toolkit à sua escolha |
| 2 | Ship it | Empacotamento, updates assinados, serviços de plataforma, a11y |
| 3 | Extensible | Plugins out-of-process, hot reload GA |
| 4 | Ecosystem | Escape hatch de webview, templates, docs |
| 5 | 1.0 | Congelamento de API, auditoria, budget de perf no CI |

## Participar

Auro está sendo desenhado em público. Arquitetura e roadmap são discutidos nas
[Discussions](https://github.com/pdl-clay/auro/discussions). As perguntas em aberto estão no fim
do [ROADMAP.md](ROADMAP.md) (em inglês). Veja o [CONTRIBUTING.md](CONTRIBUTING.md) para começar;
falhas de segurança passam pelo [SECURITY.md](SECURITY.md), nunca por uma issue pública.

Em cima dos ombros do [Tauri](https://tauri.app) e do [GPUI do Zed](https://zed.dev).

## Licença

Licenciado sob duas licenças, à sua escolha:

- [licença MIT](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

A menos que você declare explicitamente o contrário, qualquer contribuição submetida
intencionalmente para inclusão no Auro por você será licenciada da mesma forma dupla, sem termos
ou condições adicionais.
