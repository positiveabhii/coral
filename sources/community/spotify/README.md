# Spotify community source

The `spotify` community source exposes read-only Spotify profile, playlist,
library, top-item, and recent listening data through Coral SQL.

## Setup

Create a Spotify app in the Spotify Developer Dashboard:

https://developer.spotify.com/dashboard

Add this redirect URI exactly:

```text
http://127.0.0.1:53682/oauth/callback
```

Copy the app Client ID and install the source interactively:

```sh
export SPOTIFY_CLIENT_ID="<your-spotify-client-id>"
coral source add --file sources/community/spotify/manifest.yaml --interactive
```

Choose **Connect with Spotify**. Coral uses Spotify Authorization Code with
PKCE, stores the resulting access token, and sends it as
`Authorization: Bearer <token>`.

The source requests these read-only scopes:

| Scope | Used for |
| --- | --- |
| `user-read-private` | Profile country and subscription product. |
| `user-read-email` | Profile email. |
| `playlist-read-private` | Private playlists. |
| `playlist-read-collaborative` | Collaborative playlists. |
| `user-library-read` | Saved tracks and albums. |
| `user-top-read` | Top tracks and artists. |
| `user-read-recently-played` | Recent listening history. |
| `user-read-currently-playing` | Current playback item. |
| `user-read-playback-state` | Current playback queue. |

You can also paste an existing Spotify OAuth access token when adding the
source, provided it has the scopes needed for the tables you plan to query.

## Tables

| Table | Purpose | Required filters | Optional filters |
| --- | --- | --- | --- |
| `spotify.profile` | Authenticated user profile and account metadata. | — | — |
| `spotify.playlists` | Playlists owned or followed by the authenticated user. | — | — |
| `spotify.current_playback` | Current playback item. | — | `market` |
| `spotify.devices` | Spotify Connect devices visible to the account. | — | — |
| `spotify.playback_state` | Full current playback/player state. | — | `market` |
| `spotify.queue` | Current playback queue in order; first row is next. | — | — |
| `spotify.playlist_items` | Tracks and episodes in a playlist. | `playlist_id` | `market` |
| `spotify.saved_tracks` | Tracks saved in the user's library. | — | `market` |
| `spotify.saved_albums` | Albums saved in the user's library. | — | `market` |
| `spotify.saved_shows` | Shows saved in the user's library. | — | `market` |
| `spotify.saved_episodes` | Episodes saved in the user's library. | — | `market` |
| `spotify.saved_audiobooks` | Audiobooks saved in the user's library. | — | — |
| `spotify.top_tracks` | Tracks with highest affinity for the user. | — | `time_range`, `market` |
| `spotify.top_artists` | Artists with highest affinity for the user. | — | `time_range` |
| `spotify.tracks` | Track details by ID. | `id` | `market` |
| `spotify.albums` | Album details by ID. | `id` | `market` |
| `spotify.artists` | Artist details by ID. | `id` | — |
| `spotify.shows` | Show details by ID. | `id` | `market` |
| `spotify.episodes` | Episode details by ID. | `id` | `market` |
| `spotify.audiobooks` | Audiobook details by ID. | `id` | `market` |
| `spotify.album_tracks` | Tracks in an album. | `album_id` | `market` |
| `spotify.artist_albums` | Albums for an artist. | `artist_id` | `include_groups`, `market` |
| `spotify.artist_top_tracks` | Top tracks for an artist where Spotify permits the endpoint. | `artist_id` | `market` |
| `spotify.show_episodes` | Episodes in a show. | `show_id` | `market` |
| `spotify.audiobook_chapters` | Chapters in an audiobook. | `audiobook_id` | `market` |
| `spotify.recently_played` | Recent listening-history items. | — | `after`, `before` |

All tables are read-only. This source does not create, update, or delete any
Spotify data.

## Example queries

Confirm the connected account:

```sql
SELECT id, display_name, email, country, product
FROM spotify.profile;
```

List playlists and discover playlist IDs:

```sql
SELECT id, name, owner__display_name, tracks_total
FROM spotify.playlists
ORDER BY name
LIMIT 25;
```

List tracks in a playlist:

```sql
SELECT added_at, track__name, artist_names, album__name
FROM spotify.playlist_items
WHERE playlist_id = '<playlist-id>'
LIMIT 50;
```

Inspect saved tracks:

```sql
SELECT added_at, track__name, artist_names, album__name
FROM spotify.saved_tracks
ORDER BY added_at DESC
LIMIT 50;
```

Top artists over the medium-term affinity window:

```sql
SELECT time_range, name, image_url
FROM spotify.top_artists
WHERE time_range = 'medium_term'
LIMIT 20;
```


Current song or episode, when Spotify has one active:

```sql
SELECT is_playing, item__name, artist_names, album__name, progress_ms
FROM spotify.current_playback;
```


Next queued song or episode:

```sql
SELECT name, artist_names, album__name, external_url
FROM spotify.queue
LIMIT 1;
```

Recently played tracks:

```sql
SELECT played_at, track__name, artist_names, context__uri
FROM spotify.recently_played
LIMIT 50;
```

## Validation

Lint the manifest:

```sh
coral source lint sources/community/spotify/manifest.yaml
```

Install and test with real Spotify credentials:

```sh
export SPOTIFY_CLIENT_ID="<your-spotify-client-id>"
coral source add --file sources/community/spotify/manifest.yaml --interactive
coral source test spotify
```

Inspect the registered source:

```sh
coral sql "SELECT table_name, description, required_filters FROM coral.tables WHERE schema_name = 'spotify' ORDER BY table_name"
coral sql "SELECT table_name, column_name, is_required_filter FROM coral.columns WHERE schema_name = 'spotify' ORDER BY table_name, ordinal_position"
coral sql "SELECT key, kind, required, is_set FROM coral.inputs WHERE schema_name = 'spotify' ORDER BY key"
```

## Notes

- Spotify paginated collection endpoints use `limit` and `offset`; the source
  uses a default and maximum page size of 50 where Spotify supports it.
- `spotify.recently_played` returns up to 50 items and supports Spotify's
  `after`/`before` millisecond timestamp filters rather than offset pagination.
- `spotify.playlist_items` may return tracks or episodes; common item fields
  are flattened and the raw item is preserved in `raw_track`.

## Search functions

Search Spotify catalog entities with provider-ranked functions:

```sql
SELECT id, name, external_url
FROM spotify.search_tracks(query => 'Lee Morgan Sidewinder')
LIMIT 10;
```

Available search functions: `search_tracks`, `search_albums`,
`search_artists`, `search_playlists`, `search_shows`, `search_episodes`, and
`search_audiobooks`.
