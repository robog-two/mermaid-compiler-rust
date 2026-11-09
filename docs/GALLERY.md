# Gallery

Every example in `examples/diagrams/`, rendered by this implementation (no browser).
Regenerate with `cargo run --release --example gallery`.

## class

Source: [`examples/diagrams/class.mmd`](../examples/diagrams/class.mmd) · rendered in 195.80µs

<details><summary>source</summary>

```mermaid
classDiagram
    Animal <|-- Duck
    Animal <|-- Fish
    Animal <|-- Zebra
    Animal : +int age
    Animal : +String gender
    Animal: +isMammal()
    Animal: +mate()
    class Duck{
        +String beakColor
        +swim()
        +quack()
    }
    class Fish{
        -int sizeInFeet
        -canEat()
    }
    class Zebra{
        +bool is_wild
        +run()
    }
    Zoo "1" *-- "many" Animal : houses
    class Keeper {
      <<interface>>
      +feed(Animal a)
    }
    Keeper ..> Animal
```

</details>

![class](gallery/class.png)

## flowchart

Source: [`examples/diagrams/flowchart.mmd`](../examples/diagrams/flowchart.mmd) · rendered in 119.30µs

<details><summary>source</summary>

```mermaid
---
title: Order pipeline
---
flowchart TD
    A[Christmas] -->|Get money| B(Go shopping)
    B --> C{Let me think}
    C -->|One| D[Laptop]
    C -->|Two| E[iPhone]
    C -->|Three| F[fa:fa-car Car]
    D --> G[(Database)]
    E --> G
    F -.-> H((Done))
    G ==> H
    classDef hot fill:#f96,stroke:#333,stroke-width:2px
    class F hot
```

</details>

![flowchart](gallery/flowchart.png)

## pie

Source: [`examples/diagrams/pie.mmd`](../examples/diagrams/pie.mmd) · rendered in 35.16µs

<details><summary>source</summary>

```mermaid
pie showData
    title Key elements in Product X
    "Calcium" : 42.96
    "Potassium" : 50.05
    "Magnesium" : 10.01
    "Iron" :  5
```

</details>

![pie](gallery/pie.png)

## sequence

Source: [`examples/diagrams/sequence.mmd`](../examples/diagrams/sequence.mmd) · rendered in 72.73µs

<details><summary>source</summary>

```mermaid
sequenceDiagram
    title Login flow
    autonumber
    actor U as User
    participant B as Browser
    participant S as Server
    participant D as Database
    U->>B: Enter credentials
    B->>+S: POST /login
    S->>+D: SELECT user
    D-->>-S: row
    alt valid password
        S-->>B: 200 OK + cookie
    else invalid
        S--xB: 401 Unauthorized
    end
    deactivate S
    Note over B,S: TLS everywhere
    loop every 5 minutes
        B-)S: heartbeat
    end
    Note right of D: replicated
```

</details>

![sequence](gallery/sequence.png)

## state

Source: [`examples/diagrams/state.mmd`](../examples/diagrams/state.mmd) · rendered in 148.50µs

<details><summary>source</summary>

```mermaid
stateDiagram-v2
    [*] --> Still
    Still --> [*]
    Still --> Moving : push
    Moving --> Still : stop
    Moving --> Crash
    Crash --> [*]
    state Moving {
        [*] --> Slow
        Slow --> Fast : accelerate
        Fast --> Slow : brake
    }
    state check <<choice>>
    Crash --> check
    check --> Still : repaired
```

</details>

![state](gallery/state.png)

## subgraphs

Source: [`examples/diagrams/subgraphs.mmd`](../examples/diagrams/subgraphs.mmd) · rendered in 120.90µs

<details><summary>source</summary>

```mermaid
flowchart LR
    subgraph frontend [Frontend]
        ui[Web UI] --> api_client[API client]
    end
    subgraph backend [Backend]
        direction TB
        api[REST API] --> svc[Service]
        svc --> db[(Postgres)]
        subgraph workers [Workers]
            q[/Queue/] --> w1[Worker 1]
            q --> w2[Worker 2]
        end
        svc --> q
    end
    api_client --> api
    w1 --> db
    w2 --> db
    user((User)) --> ui
```

</details>

![subgraphs](gallery/subgraphs.png)

