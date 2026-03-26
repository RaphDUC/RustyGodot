# TP2 — Entrées réseau & mesure de latence

**Cours :** Architectures réseau pour jeux  
**Session :** Hiver 2025  
**Remise :** 2 semaines après la séance de TP  
**Prérequis :** TP1 fonctionnel (serveur UDP + client connecté)

## Contexte et objectifs

Dans le TP1, vous avez mis en place un serveur de jeu UDP (non fiable) et un client capable de s'y connecter. Le but était d'établir le canal de communication. Dans ce TP2, vous allez **donner vie à ce joueur** en faisant transiter leur inputs.

À l'issue de ce TP, votre application devra être capable de :

- Capturer les entrées clavier/souris du joueur côté client.
- Les sérialiser et les envoyer au serveur via le socket UDP du TP1.
- Mesurer la latence réseau aller-retour (RTT) entre le client et le serveur à l'aide d'une approche inspirée du protocole NTP.
- Afficher la latence estimée en temps réel dans la console ou dans l'interface.


## Partie A — Transmission des entrées joueur

### Format du paquet d'entrée

Définissez une structure d'entrée simple. Elle devra contenir au minimum :

- Un identifiant de séquence (`u32`) pour détecter les pertes et ordonner les paquets.
- Les flags de touches actives (`u8` ou bitfield) : haut, bas, gauche, droite, action.
- La direction visée ou la position de la souris (optionnel mais recommandé).

Exemple en Rust :

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputPacket {
    pub sequence:  u32,
    pub keys:      u8,    // bitfield : bit0=haut, bit1=bas, bit2=gauche, bit3=droite
    pub aim_x:     f32,
    pub aim_y:     f32,
}
```

Afin de garantir la réception envoyez un historique de 20 inputs dans chaque paquets.

### Envoi et réception côté serveur

Le serveur doit :

- Recevoir les paquets `InputPacket` sur le même socket UDP du TP1.
- Désérialiser et utiliser chaque paquet (séquence + touches actives).
- Détecter les paquets hors-ordre à l'aide du numéro de séquence et les ignorer.

## Partie B — Mesure de latence (approche NTP)

### Principe du protocole NTP

Le protocole **NTP (Network Time Protocol)** permet à un client de synchroniser son horloge avec un serveur tout en estimant le délai de transit réseau. Le mécanisme de base repose sur un échange de quatre horodatages :

```
CLIENT                                      SERVEUR
  |                                            |
  |  ── PingRequest { t1 } ──────────────────> | 
  |                                            |
  |  <── PingResponse { t1, t2 } ─────── |     | t2 = départ réponse
  |                                            |
  t3 temps d'arrivée
```

À la réception de la réponse, le client dispose de quatre horodatages :

| Variable | Horloge | Signification |
|---|---|---|
| `t1` | Client | Instant d'envoi de la requête Ping |
| `t2` | Serveur | Instant d'envoi de la réponse Pong |
| `t3` | Client | Temps de réception du paquet coté client |

### Formules de calcul

**RTT (Round-Trip Time)**

```
RTT = (t3 − t1)
```

### Implémentation

Définissez deux types de paquets supplémentaires :

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct PingRequest {
    pub id: u32,   // identifiant de la sonde
    pub t0: u64,   // timestamp client (ms) à l'envoi
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PingResponse {
    pub id: u32,
    pub t0: u64,   // recopié depuis la requête
    pub t1: u64,   // timestamp serveur à l'envoi
}
```

Côté client, dans un thread ou une tâche async :

```rust
async fn ping_loop(socket: Arc<UdpSocket>, server_addr: SocketAddr) {
    let mut id = 0u32;
    loop {
        let t0 = now_ms();
        let req = PingRequest { id, t0 };
        socket.send_to(&serialize(&req), server_addr).await.unwrap();

        // Attendre la réponse (avec timeout)
        let mut buf = vec![0u8; 256];
        if let Ok((n, _)) = timeout(
            Duration::from_secs(2),
            socket.recv_from(&mut buf)
        ).await.unwrap_or(Err(io::Error::new(io::ErrorKind::TimedOut, ""))) {
            let resp: PingResponse = deserialize(&buf[..n]).unwrap();
            let t2 = now_ms();
            let rtt = (t2 - resp.t0);
            println!("[Ping #{id}]  RTT = {rtt} ms");
        }
        id += 1;
        sleep(Duration::from_secs(1)).await;
    }
}
```

Côté serveur, répondez immédiatement à un `PingRequest` :

```rust
let ping: PingRequest = deserialize(&buf[..n])?;
let t1 = now_ms();
let response = PingResponse { id: ping.id, t0: ping.t0, t1 };
socket.send_to(&serialize(&response), src).await?;
```