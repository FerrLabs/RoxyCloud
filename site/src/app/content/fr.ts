import type { SiteContent } from './site-content';

export const fr: SiteContent = {
  chrome: {
    skipToContent: 'Aller au contenu',
    nav: { overview: 'Présentation', install: 'Installation', api: 'API' },
    languageName: 'Français',
    languageSwitch: 'Read in English',
    footer: {
      tagline: 'Stockage de fichiers auto-hébergé, écrit en Rust.',
      licence: 'AGPL-3.0',
      source: 'Sources',
      parent: 'Un projet FerrLabs',
    },
  },

  home: {
    documentTitle: 'RoxyCloud, stockage de fichiers auto-hébergé en Rust',
    description:
      "RoxyCloud est un serveur de fichiers auto-hébergé écrit en Rust : une application web, une API REST et WebDAV au-dessus d'un stockage adressé par contenu, sous licence AGPL.",
    eyebrow: 'Stockage de fichiers auto-hébergé',
    heading: 'Vos fichiers, sur votre machine.',
    lead: "RoxyCloud est un serveur de fichiers écrit en Rust. Il range ce que vous envoyez dans un stockage adressé par contenu, le sert via une API REST et une application web, et tourne sur une machine qui vous appartient.",
    install: "L'installer",
    source: 'Lire les sources',
    status: {
      heading: 'Où en est vraiment le projet',
      lead: "Jeune, et pas encore utilisable de bout en bout. Voici le partage, pour que vous décidiez de le faire tourner maintenant ou de l'observer encore un peu.",
      shippedHeading: "Ce qui fonctionne aujourd'hui",
      shipped: [
        "Un stockage local adressé par contenu, qui déduplique les envois identiques",
        "L'arbre des nœuds, avec les quotas par utilisateur et le comptage de références sur les blobs",
        'Envoi, téléchargement, listage, corbeille et restauration via REST',
        "Des comptes par mot de passe en Argon2id, des jetons de session, et la connexion depuis l'application web, la fenêtre desktop et la ligne de commande",
      ],
      plannedHeading: "Ce qui n'est pas encore écrit",
      planned: [
        "Les mots de passe applicatifs, et le WebDAV pour lequel ils existent",
        'Le partage par lien, et la recherche par nom',
        'La connexion OIDC',
        "Le backend S3, et le ramasse-miettes qui récupère les blobs orphelins",
        'Le moteur de synchronisation derrière le client desktop',
      ],
    },
    design: {
      heading: 'Comment il est construit',
      items: [
        {
          title: 'Un stockage adressé par contenu',
          body: "Un fichier est rangé sous l'empreinte de ses octets : la même pièce jointe envoyée deux fois ne coûte qu'une copie. Le comptage de références sur l'arbre décide quand ces octets peuvent partir.",
        },
        {
          title: 'Une interface, deux hôtes',
          body: "L'application web et la fenêtre desktop exécutent le même build Angular. Ce qui change entre les deux n'est pas l'interface mais ce qu'elle a le droit d'atteindre, et cela tient dans un seul fichier.",
        },
        {
          title: 'Un binaire et une base',
          body: "Les migrations tournent au démarrage, la configuration est uniquement dans l'environnement, et il n'y a pas de système de plugins à sécuriser. Mettre à jour, c'est remplacer l'image.",
        },
        {
          title: "Le lien vers les sources n'est pas décoratif",
          body: "RoxyCloud est sous AGPL-3.0. Faites tourner une version modifiée pour d'autres et ils ont le droit de la lire, d'où le lien que l'application web porte vers les sources du build qu'elle sert.",
        },
      ],
    },
  },

  install: {
    documentTitle: 'Installer RoxyCloud',
    description:
      "Faire tourner RoxyCloud avec Docker Compose ou depuis les sources : prérequis, variables d'environnement, premier administrateur et build de l'application web.",
    heading: 'Installer RoxyCloud',
    lead: "Deux chemins. Docker Compose pour qu'il réponde sur le port 3001 en quelques minutes, une chaîne d'outils Rust si vous comptez le modifier.",
    requirements: {
      heading: 'Avant de commencer',
      items: [
        "Postgres 15 ou plus récent. Compose fournit le sien, vous n'avez donc à en fournir un que sur le chemin des sources.",
        "Une chaîne d'outils Rust correspondant à rust-toolchain.toml, pour le chemin des sources.",
        "Node 24 et pnpm, pour construire l'application web.",
      ],
    },
    compose: {
      heading: 'Avec Docker Compose',
      body: "Clonez le dépôt, définissez les secrets sans lesquels Compose refuse de démarrer, et lancez-le. L'image compile l'API depuis les sources, le premier démarrage prend donc quelques minutes.",
      blocks: [
        {
          caption: 'Cloner et configurer',
          code: `git clone https://github.com/FerrLabs/RoxyCloud.git
cd RoxyCloud

export POSTGRES_PASSWORD='une longue chaîne aléatoire'
export JWT_SECRET='une autre longue chaîne aléatoire'
export BOOTSTRAP_ADMIN_EMAIL='vous@exemple.com'
export BOOTSTRAP_ADMIN_PASSWORD='au moins douze caractères'`,
        },
        {
          caption: "Le démarrer, et vérifier qu'il répond",
          code: `docker compose -f deploy/docker-compose.yml up -d --build
curl --fail http://localhost:3001/health`,
        },
      ],
      note: "Les deux variables de bootstrap ne servent que sur une base vide, où elles créent le premier administrateur. Retirez-les de l'environnement une fois que c'est fait.",
    },
    source: {
      heading: 'Depuis les sources',
      body: "Le serveur lit sa configuration dans l'environnement et joue ses migrations au démarrage : une base et un secret de signature suffisent à le lever.",
      blocks: [
        {
          caption: 'Lancer l’API sur un Postgres local',
          code: `DATABASE_URL=postgres://localhost/roxycloud \\
JWT_SECRET=dev-secret \\
cargo run -p roxycloud-api`,
        },
      ],
    },
    configuration: {
      heading: 'Configuration',
      body: "Uniquement l'environnement. Aucun fichier de configuration à monter, et aucune page d'administration qui en écrive un dans votre dos.",
      columns: ['Variable', 'Défaut', 'Rôle'],
      rows: [
        ['DATABASE_URL', 'obligatoire', 'Chaîne de connexion Postgres'],
        ['JWT_SECRET', 'obligatoire', 'Secret HS256 utilisé pour signer les jetons de session'],
        ['PORT', '3001', "Port d'écoute"],
        ['BLOB_ROOT', './data', 'Racine du stockage local'],
        [
          'WEB_ROOT',
          "défini dans l'image",
          "Répertoire contenant l'application web compilée, servie à côté de l'API",
        ],
        [
          'CORS_ALLOWED_ORIGINS',
          'vide',
          "Origines séparées par des virgules autorisées à appeler l'API depuis un navigateur, inutile quand WEB_ROOT la sert",
        ],
        ['DEFAULT_QUOTA_BYTES', '10 Gio', 'Quota accordé à un compte à sa première écriture'],
        ['SESSION_TTL_SECONDS', '12 h', 'Durée de vie du jeton de session'],
        [
          'BLOB_SWEEP_INTERVAL_SECONDS',
          '1 h',
          'Fréquence de collecte des blobs que plus rien ne référence, 0 la désactive',
        ],
        [
          'BLOB_GRACE_PERIOD_SECONDS',
          '24 h',
          "Durée de conservation d'un blob déréférencé avant sa collecte",
        ],
        ['BOOTSTRAP_ADMIN_EMAIL', 'non défini', 'Crée le premier administrateur sur une base vide'],
        [
          'BOOTSTRAP_ADMIN_PASSWORD',
          'non défini',
          "Obligatoire avec l'email, douze caractères minimum",
        ],
      ],
    },
    firstLogin: {
      heading: 'La première connexion',
      body: "La ligne de commande vit dans le même workspace et parle à la même API, ce qui en fait le moyen le plus court de vérifier que l'administrateur existe.",
      blocks: [
        {
          caption: 'Se connecter en ligne de commande',
          code: `cargo run -p roxycloud-cli -- login vous@exemple.com --password '...'`,
        },
      ],
    },
    webApp: {
      heading: "L'application web",
      body: "L'image la transporte, et l'API la sert depuis la même origine : rien à déployer à côté, aucun CORS à configurer. L'héberger vous-même reste possible : compilez web/dist avec l'adresse de votre API, servez-la depuis n'importe quel hébergement statique, et déclarez son origine dans CORS_ALLOWED_ORIGINS. Profitez-en pour y compiler les sources de la version que vous faites réellement tourner.",
      blocks: [
        {
          caption: "Construire l'interface navigateur pour votre propre hébergement",
          code: `pnpm install
pnpm --filter @roxycloud/web build \\
  --define ROXYCLOUD_API_URL="'https://fichiers.exemple.com'" \\
  --define ROXYCLOUD_SOURCE_URL="'https://git.exemple.com/roxycloud'"`,
        },
      ],
    },
  },

  api: {
    documentTitle: "L'API RoxyCloud",
    description:
      "L'API REST de RoxyCloud : jetons de session, endpoints dossiers et fichiers, et ce qui n'est pas encore implémenté.",
    heading: "L'API",
    lead: "Une seule surface REST, du JSON dans les deux sens, des jetons bearer. WebDAV arrivera sur le même binaire sous /dav et n'est pas encore écrit.",
    session: {
      heading: 'Obtenir un jeton',
      body: "Toutes les routes /v1 sauf la connexion attendent un en-tête Authorization. Une session dure douze heures sauf si SESSION_TTL_SECONDS en décide autrement, et il n'y a pas de rafraîchissement : reconnectez-vous. Le compte porte un rôle, admin, member ou reader, et un lecteur reçoit un 403 à l'envoi et à la suppression plutôt qu'une interface qui cache les boutons.",
      blocks: [
        {
          caption: 'Échanger un mot de passe contre une session',
          code: `curl -X POST http://localhost:3001/v1/auth/login \\
  -H 'Content-Type: application/json' \\
  -d '{"email":"vous@exemple.com","password":"..."}'`,
        },
        {
          caption: 'Ce qui revient',
          code: `{
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_in": 43200,
  "user": {
    "id": "b7f0c2de-6a1e-4d5f-9a0b-2f9c1d7e4a30",
    "email": "vous@exemple.com",
    "display_name": "vous",
    "role": "admin",
    "is_admin": true,
    "created_at": "2026-09-03T08:00:00Z"
  }
}`,
        },
      ],
    },
    endpoints: {
      heading: 'Endpoints',
      body: "C'est la surface complète, pas une sélection.",
      columns: ['Méthode', 'Chemin', 'Ce que ça fait'],
      rows: [
        ['GET', '/health', 'Vérification de vie, la seule route sans jeton'],
        ['POST', '/v1/auth/login', 'Échanger un email et un mot de passe contre un jeton de session'],
        ['GET', '/v1/auth/me', 'Le compte authentifié'],
        ['GET', '/v1/folders', 'Lister la racine'],
        ['GET', '/v1/folders/{*path}', 'Lister un répertoire'],
        ['PUT', '/v1/files/{*path}', 'Envoyer, en créant les répertoires parents'],
        ['GET', '/v1/files/{*path}', 'Télécharger'],
        ['DELETE', '/v1/files/{*path}', 'Mettre à la corbeille'],
        ['POST', '/v1/move', 'Renommer un nœud, ou le déplacer sous un autre répertoire'],
        ['GET', '/v1/trash', "Ce que le compte a supprimé"],
        ['POST', '/v1/trash/{id}/restore', 'Le restaurer, avec les répertoires nécessaires'],
        ['DELETE', '/v1/trash/{id}', 'Le supprimer définitivement et libérer ses octets'],
      ],
    },
    transfers: {
      heading: 'Déplacer des octets',
      body: "Un envoi, c'est le fichier brut dans le corps de la requête, et il répond 201 avec le nœud et un ETag. Un téléchargement renvoie les octets en flux avec ce même ETag, et une suppression répond 204 une fois le nœud à la corbeille.",
      blocks: [
        {
          caption: 'Envoyer un fichier',
          code: `curl -X PUT http://localhost:3001/v1/files/notes/todo.md \\
  -H "Authorization: Bearer $TOKEN" \\
  --data-binary @todo.md`,
        },
        {
          caption: 'Renommer un fichier, puis le déplacer dans un répertoire',
          code: `curl -X POST http://localhost:3001/v1/move \\
  -H "Authorization: Bearer $TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{"from": "/brouillon.md", "to": "/a-faire.md"}'

curl -X POST http://localhost:3001/v1/move \\
  -H "Authorization: Bearer $TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{"from": "/a-faire.md", "to": "/notes/a-faire.md"}'`,
        },
        {
          caption: 'Lister un répertoire, puis y télécharger',
          code: `curl http://localhost:3001/v1/folders/notes \\
  -H "Authorization: Bearer $TOKEN"

curl -O http://localhost:3001/v1/files/notes/todo.md \\
  -H "Authorization: Bearer $TOKEN"`,
        },
      ],
    },
    gaps: {
      heading: 'Ce qui manque',
      body: "Chercher, partager par lien, les mots de passe applicatifs, WebDAV et les envois reprenables sont suivis en issues, et aucun n'est implémenté. Un chemin absent du tableau ci-dessus répond 404.",
    },
  },
};
