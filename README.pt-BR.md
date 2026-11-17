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

**Pré-alpha. Projetando em público.** O workspace já existe e a fase 0 está concluída; nada foi
publicado no crates.io e ainda não há CLI lançada. Dá para compilar e experimentar o framework a
partir de um checkout (veja [CONTRIBUTING.md](CONTRIBUTING.md)).

| Fase | Codinome | Estado |
|---|---|---|
| 0 | Foundations | **concluída** |
| 1 | Hello, native | planejada |
| 2 | Ship it | planejada |
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
| RAM idle (hello world) | ~180 MB | ~90 MB | meta **< 40 MB** |
| Cold start | ~900 ms | ~400 ms | meta **< 80 ms** |
| Binário | ~120 MB | ~3-10 MB | meta **< 6 MB** |
| Texto/IME/seleção | do navegador | do navegador | herdado de um editor |

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

| Métrica | Meta no 1.0 |
|---|---|
| Cold start | < 80 ms |
| RAM idle | < 40 MB |
| Binário hello-world | < 6 MB (stripped) |
| Editar e ver | < 500 ms |
| Plataformas | macOS 12+, Windows 10+, Ubuntu 22.04+ (Wayland e X11) |

Medido no CI. Regressão quebra o build.

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
