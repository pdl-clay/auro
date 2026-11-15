# Auro

[English](README.md) · **Português**

**O modelo de app do Tauri. O renderer do GPUI. Zero webview.**

Auro é um framework de aplicação para apps desktop em Rust. Ele pega as partes do Tauri que
dão trabalho para reconstruir — configuração, capabilities, plugins, bundler, updater assinado,
dev server, single-instance, multi-window — e troca o renderer: em vez de uma webview, a UI é o
GPUI, o toolkit de UI em Rust acelerado por GPU do Zed.

O frontend é Rust. Não existe HTML, nem engine de CSS, nem JavaScript no runtime do app.

> **Status: pré-alpha.** Projetando em público. Veja o [ROADMAP.md](ROADMAP.md) (em inglês).

---

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
```

Sem `invoke("save_document", {...})`. O comando é uma função Rust; o ponto de chamada é uma
chamada Rust. Erro de tipo é erro de compilação.

## As três decisões que definem o projeto

1. **IPC tipado, em tudo.** Os schemas saem das assinaturas Rust. Não existe ponte
   stringly-typed, porque não existe JavaScript para atravessar.
2. **Hot reload ou nada.** O tempo de compilação do Rust é o único motivo real para continuar na
   web. Meta: editar e ver em **menos de 500 ms**, com o estado do app preservado.
3. **Capabilities na fronteira de verdade.** Com frontend em Rust, a maior parte da checagem de
   permissão é em tempo de compilação. O que realmente precisa de cerca em runtime é **código de
   plugin** e **conteúdo não confiável**. É aí que mora o modelo de capabilities do Auro.

## O que o Auro não é

- **Não é wrapper de webview.** Webview só existe como escape hatch com escopo explícito para
  OAuth e docs embutidos; não é a fundação.
- **Não é toolkit de GUI.** Isso é o GPUI. Auro é framework de *aplicação*.
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
| 1 | Hello, native | Core + 20 componentes + `auro dev` |
| 2 | Ship it | Empacotamento, updates assinados, serviços de plataforma, a11y |
| 3 | Extensible | Plugins out-of-process, hot reload GA |
| 4 | Ecosystem | Escape hatch de webview, templates, docs |
| 5 | 1.0 | Congelamento de API, auditoria, budget de perf no CI |

## Participar

Auro está sendo desenhado em público. Arquitetura e roadmap são discutidos nas Discussions do
repositório. As perguntas em aberto estão no fim do [ROADMAP.md](ROADMAP.md).

Em cima dos ombros do Tauri e do GPUI do Zed.

## Licença

Dual: MIT ou Apache-2.0, à sua escolha.
