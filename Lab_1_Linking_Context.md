# Laboratoire 1 : Architecture de Réplication Réseau (20%)
**Sujet :** Implémentation du « Linking Context » et Réplication Basique  
**Moteurs & Outils :** Godot 4.x, C++ (EnTT) ou Rust (Bevy), SNL (Stupid Network Library)

## Contexte et Objectifs

Dans ce cours, nous ne nous contentons pas d'utiliser des solutions clé en main ; nous construisons les nôtres. L'objectif de ce premier laboratoire est de poser les fondations de votre *Lightweight Replication Framework*.

Le défi principal du réseau dans les jeux vidéo est de maintenir une cohérence entre des espaces mémoire distincts. Le serveur a ses propres pointeurs, et chaque client a les siens. Pour communiquer, ils doivent partager un langage commun : le **Linking Context**.

### Objectifs du laboratoire :
1.  Comprendre et implémenter un système d'identification unique (**NetworkID**).
2.  Créer un registre de types (**TypeID**) associé à des fonctions de fabrication (**Creation Lambdas**).
3.  Établir la communication basique entre un serveur (C++ ou Rust) et des clients (Godot) via la librairie **SNL**.
4.  Répliquer la création d'une entité (Spawn) du serveur vers tous les clients connectés.

## Architecture Technique : Le « Linking Context »

Le *Linking Context* est la pierre angulaire de votre architecture réseau. C'est un registre bidirectionnel qui permet de traduire un identifiant réseau (reçu dans un paquet) en une instance locale concrète (un nœud Godot ou une entité ECS).

Il doit gérer trois concepts clés :
* **NetworkID (uint32_t) :** L'identifiant unique de l'objet sur le réseau. Le serveur fait autorité sur sa génération.
* **TypeID (uint32_t) :** L'identifiant du "type" d'objet (ex: `1` = Joueur, `2` = Ennemi, `3` = Projectile).
* **Creation Lambda :** Une fonction anonyme ou un pointeur de fonction capable d'instancier l'objet localement si celui-ci n'existe pas encore.

### Structure de Données Suggérée
Votre structure doit permettre une recherche rapide ($O(1)$ ou proche).

#### C++ (Approche EnTT)
```cpp
struct ObjectContext {
    uint32_t network_id;
    entt::entity local_entity;
};
// Une map pour retrouver l'objet local via son ID réseau
std::unordered_map<uint32_t, ObjectContext> network_to_local_map;
```

#### Rust (Approche Bevy)

```rust
#[derive(Resource)]
struct NetworkMapping {
    // Mapping NetworkID -> Entity Bevy
    map: HashMap<u32, Entity>,
}
```

## Protocole et Paquets
Pour ce laboratoire, nous définissons un protocole binaire minimaliste. La librairie SNL envoie des paquets bruts (raw bytes).

### Structure du paquet « SPAWN »

Ce paquet est envoyé par le serveur pour informer les clients qu'une nouvelle entité existe.

TABLE

## Travail à Réaliser

### Étape A : Le Serveur (Backend C++ ou Rust)

- Initialisation SNL : Utilisez SNL::bind(port) pour écouter les connexions entrantes.
- Boucle Principale (Game Loop) :
    - Appelez SNL::receive de manière non-bloquante.
    - Détectez les nouvelles connexions.
    - Lorsqu'un client se connecte, créez une entité dans votre ECS serveur (EnTT ou Bevy).
    - Attribuez-lui un NetworkID (incrémental, ex: 100, 101...).
    - Construisez un paquet SPAWN et envoyez-le à tous les clients via SNL::send.

### Étape B : Le Client (Godot + GDExtension/Rust Bindings)

- Le Registre (LinkingContext) :
    - Au démarrage, enregistrez vos types.
    - Exemple conceptuel : LinkingContext::RegisterType(1, []() { return new PlayerScene(); });

- Réception Réseau :
    - Dans _process ou _physics_process, poll le socket via SNL::receive.
    - Si un paquet est reçu, lisez le type_id et le network_id.

- Instanciation :
    - Vérifiez si le network_id existe déjà dans votre map.
    - Si NON :
        - Trouvez la Lambda de création associée au type_id.
        - Exécutez la lambda pour obtenir le Node Godot.
        - Ajoutez le Node à l'arbre de scène (add_child).
        - Enregistrez la paire (network_id, node_reference) dans votre map.
    - Si OUI : (Pour les futurs labos) Mettez à jour la position.

## Critères de Validation
Pour valider ce laboratoire, vous devez faire la démonstration suivante :

- Lancement Multi-processus :
    - Lancer 1 Serveur (Console).
    - Lancer 2 Clients Godot distincts.

- Scénario de Test :
    - Le Client A se connecte.
    - Résultat attendu :
        - Le Serveur affiche : [Server] Client A connected. Spawning Entity ID 100.
        - Le Client A voit apparaître son personnage (et potentiellement ceux déjà présents).
        - Le Client B (s'il est déjà connecté) voit apparaître le personnage du Client A instantanément.

- Inspection Mémoire :
    - Prouvez via des logs que le NetworkID est identique sur les 3 processus (Serveur, Client A, Client B), malgré le fait que les adresses mémoires locales soient différentes.

## Notes Importantes sur SNL
La librairie SNL (fournie dans le dépôt du cours) est minimaliste.
    - Non-blocking I/O : receive retournera immédiatement s'il n'y a pas de données. Vous ne devez pas faire de while(true) bloquant. Gérez le cas "No Data" proprement.
    - Endianness : Pour ce labo, nous supposons que toutes les machines sont en Little Endian (architecture x86_64 standard). Ne vous souciez pas de la sérialisation complexe pour l'instant, envoyez les structs brutes (memcpy est toléré pour ce premier labo uniquement).

**Bon code !**