How to reproduce what's currently working:

First create MariaDB & discord bot:
```
cd docker
docker compose up
```

Create database:
```
cd docker
./mariadb.sh
create database linked_players;
```

Compile 6b6t-plugins - DiscordManager

```
./gradlew :main:DiscordManager:build
```

And put it on the test server. Configure MySQL in plugins/DiscordManager/config.yml - IP and port of the host machine port: 33123 user: root pwd: devenv
Restart the server.

Afterwards go to: https://discord.com/channels/1326869396324614245/1326869396324614248
Click on "Link your account"

Copy-paste the code into the game

The code is valid for 5 minutes.
The account linking isn't implemented, it basically only displays a green "yes, linked" or red "wrong, invalid code".

What I think should be implemented is kinda a "canonical table" that is a ground truth for linking Discord players -> player UUID.
Afterwards all other linkings/Discord users/players will be validated based on this array.