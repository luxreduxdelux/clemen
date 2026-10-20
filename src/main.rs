use chrono::{DateTime, Local};
use lastfm_client::LastFmClient;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use teloxide::types::{InputPollOption, MessageId, ParseMode, Recipient};
use teloxide::utils::command::BotCommands;
use teloxide::{prelude::*, types::InputFile};
use tmdb_api::client::Client;
use tmdb_api::client::reqwest::ReqwestExecutor;
use tmdb_api::movie::search::MovieSearch;
use tmdb_api::prelude::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;
use yt_dlp::{Downloader, model::Video};

//================================================================

struct State {
    data: Data,
    tube: YouTube,
    tmdb: Client<ReqwestExecutor>,
    last: LastFmClient,
}

impl State {
    async fn new() -> anyhow::Result<Self> {
        Ok(Self {
            data: Data::default(),
            tube: YouTube::new().await?,
            tmdb: Client::<ReqwestExecutor>::new(include_str!("../clemen_tmdb.env").to_string()),
            last: LastFmClient::builder()
                .api_key(include_str!("../clemen_last.env"))
                .timeout(Duration::from_secs(60))
                .max_concurrent_requests(5)
                .build_client()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Upload {
    message: i32,
    time: DateTime<Local>,
}

impl Upload {
    fn new(message: i32) -> Self {
        Self {
            message,
            time: Local::now(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    music: Vec<Upload>,
    queue: Vec<Proposal>,
}

impl Default for Data {
    fn default() -> Self {
        if let Ok(data) = std::fs::read_to_string(Self::FILE_PATH) {
            serde_json::from_str(&data).expect("Couldn't deserialize clemen.json.")
        } else {
            Data {
                music: Vec::default(),
                queue: Vec::default(),
            }
        }
    }
}

impl Data {
    const VOTE_USER: u64 = 1511061836;
    const FILE_PATH: &str = "clemen.json";
    const CHANNEL_MUSIC: i64 = -1001296790112;
    const CHANNEL_DEBUG: i64 = -1003792156195;

    fn add(&mut self, name: String, user: String) -> bool {
        for p in &self.queue {
            if p.name.to_lowercase().trim() == name.to_lowercase().trim() {
                return false;
            }
        }

        self.queue.push(Proposal::new(name, user));
        self.save();
        true
    }

    fn clear(&mut self) -> Vec<Proposal> {
        let queue = self.queue.clone();
        self.queue.clear();
        self.save();

        queue
    }

    fn add_upload(&mut self, upload: Upload) {
        self.clean_upload();
        self.music.push(upload);
        self.save();
    }

    fn clean_upload(&mut self) {
        let time = Local::now().date_naive();
        self.music
            .retain(|upload| (time - upload.time.date_naive()).num_days() <= 29);
    }

    fn upload_range(&mut self, range: i64) -> Vec<MessageId> {
        self.clean_upload();
        let mut result = Vec::new();
        let time = Local::now().date_naive();

        for upload in &self.music {
            if (time - upload.time.date_naive()).num_days() <= range {
                result.push(MessageId(upload.message));
            }
        }

        result
    }

    fn save(&self) {
        let data = serde_json::to_string_pretty(self).expect("Couldn't serialize clemen.json");
        std::fs::write(Self::FILE_PATH, data).expect("Couldn't save clemen.json.");
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Proposal {
    name: String,
    user: String,
}

impl Proposal {
    fn new(name: String, user: String) -> Self {
        Self { name, user }
    }
}

struct YouTube {
    downloader: Downloader,
}

impl YouTube {
    async fn new() -> anyhow::Result<Self> {
        let downloader = Downloader::with_new_binaries("clemen_binary", "clemen_out")
            .await?
            .build()
            .await?;

        Ok(Self { downloader })
    }

    async fn download(&self, link: &str) -> anyhow::Result<Video> {
        let video = self.downloader.fetch_video_infos(link).await?;

        // I think a YouTube update made it so no YouTube Music video actually
        // has a video stream so it broke download_video_with_quality, so do this
        // check to see if it's a YTM video and only download the audio stream
        if Self::try_artist_track(&video).2 {
            self.downloader
                .download_audio_stream_with_quality(
                    &video,
                    "audio.mp4",
                    //yt_dlp::model::VideoQuality::Worst,
                    //yt_dlp::model::VideoCodecPreference::Any,
                    yt_dlp::model::AudioQuality::Best,
                    yt_dlp::model::AudioCodecPreference::Any,
                )
                .await?;
        } else {
            self.downloader
                .download_video_with_quality(
                    &video,
                    "audio.mp4",
                    yt_dlp::model::VideoQuality::Worst,
                    yt_dlp::model::VideoCodecPreference::Any,
                    yt_dlp::model::AudioQuality::Best,
                    yt_dlp::model::AudioCodecPreference::Any,
                )
                .await?;
        }

        Ok(video)
    }

    fn try_artist_track(video: &Video) -> (String, Option<String>, bool) {
        // Try getting the artist from a YouTube-made music video.
        if let Some(description) = &video.description
            && description.contains("Auto-generated by YouTube.")
            && let Some(channel) = &video.channel
        {
            let channel: Vec<&str> = channel.split("-").collect();
            let channel = channel.first().unwrap();

            return (channel.to_string(), Some(video.title.clone()), true);
        }

        let split: Vec<&str> = video.title.split("-").collect();

        if split.len() >= 2 {
            (
                split[0].trim().to_string(),
                Some(split[1].trim().to_string()),
                false,
            )
        } else {
            (video.title.clone(), Some("".to_string()), false)
        }
    }

    fn get_link_list(text: &str) -> Vec<String> {
        let expression = Regex::new(
            r"\b((?:https?:\/\/)?(?:www\.)?[a-zA-Z0-9-]+(?:\.[a-zA-Z0-9-]+)+(?:\/[^\s]*)?)\b",
        )
        .unwrap();

        let mut list = Vec::default();
        for (_, [link]) in expression
            .captures_iter(text)
            .map(|capture| capture.extract())
        {
            list.push(link.to_string());
        }

        list
    }
}

//================================================================

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    Proponer(String),
    Desproponer(String),
    Sinopsis(String),
    Cola,
    Votar,
    Dia,
    Semana,
    Mes,
    LastUsuario(String),
    CancionSemanal(String),
    ArtistaSemanal(String),
    AlbumSemanal(String),
    Vivo,
}

async fn handle_command(
    bot: Bot,
    message: Message,
    command: BotCommand,
    state: Arc<Mutex<State>>,
) -> ResponseResult<()> {
    match command {
        BotCommand::Proponer(name) => {
            if let Some(user) = message.from {
                if name.is_empty() {
                    bot.send_message(message.chat.id, "Tenés que dar el nombre de la película. Ejemplo: /proponer@clemen_dc_bot Rocky Horror Picture Show")
                        .await?;
                } else {
                    let mut state = state.lock().await;

                    if state.data.queue.len() == 12 {
                        bot.send_message(message.chat.id, "Telegram tiene un límite de 12 opciones por encuesta...don't hate the player hate the game").await?;
                    } else {
                        if state.data.add(name, user.full_name()) {
                            bot.send_message(message.chat.id, "Lo anotubi").await?;
                        } else {
                            bot.send_message(
                                message.chat.id,
                                "Ya está en la cola, me estás gargando",
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        BotCommand::Desproponer(name) => {
            if let Some(user) = message.from {
                if name.is_empty() {
                    bot.send_message(message.chat.id, "Tenés que dar el nombre de la película. Ejemplo: /desproponer@clemen_dc_bot Rocky Horror Picture Show")
                        .await?;
                } else {
                    let mut state = state.lock().await;

                    for (i, proposal) in state.data.queue.iter().enumerate() {
                        if proposal.name == name {
                            if user.full_name() == proposal.user {
                                state.data.queue.remove(i);
                                state.data.save();
                                bot.send_message(message.chat.id, "Lo desanotubi").await?;
                            } else {
                                bot.send_message(message.chat.id, "Me estás gargando...no propusiste esa película vos. 🍅la de acá")
                                    .await?;
                            }
                            break;
                        }
                    }
                }
            }
        }
        BotCommand::Sinopsis(name) => {
            if name.is_empty() {
                bot.send_message(message.chat.id, "Tenés que dar el nombre de la película. Ejemplo: /sinopsis@clemen_dc_bot Rocky Horror Picture Show")
                        .await?;
            } else {
                if let Ok(result) = MovieSearch::new(name.into())
                    .execute(&state.lock().await.tmdb)
                    .await
                {
                    if let Some(movie) = result.results.first() {
                        bot.send_message(
                            message.chat.id,
                            format!(
                                "{} • ({})\n\n{}\n\nhttps://www.themoviedb.org/movie/{}",
                                movie.inner.title,
                                movie.inner.release_date.unwrap_or_default(),
                                movie.inner.overview,
                                movie.inner.id
                            ),
                        )
                        .await?;
                    } else {
                        bot.send_message(
                            message.chat.id,
                            "No encontré ningún resultado para esa película.",
                        )
                        .await?;
                    }
                }
            }
        }
        BotCommand::Cola => {
            let queue = state.lock().await.data.queue.clone();
            let mut text = String::default();

            for p in &queue {
                text.push_str(&format!("• {}, propuesta por {}\n", p.name, p.user));
            }

            if text.is_empty() {
                bot.send_message(message.chat.id, "La cola está vacía.")
                    .await?;
            } else {
                bot.send_message(message.chat.id, text).await?;
            }
        }
        BotCommand::Votar => {
            if let Some(user) = message.from {
                if user.id.0 == Data::VOTE_USER {
                    let queue = state.lock().await.data.clear();
                    let mut vote = Vec::default();

                    for p in &queue {
                        vote.push(InputPollOption {
                            text: p.name.clone(),
                            formatting: None,
                        });
                    }

                    if vote.is_empty() {
                        bot.send_message(message.chat.id, "La cola está vacía.")
                            .await?;
                    } else {
                        bot.send_poll(message.chat.id, "Vota la próxima película!", vote)
                            .type_(teloxide::types::PollType::Regular)
                            .is_anonymous(false)
                            .allows_multiple_answers(true)
                            .await?;
                    }
                } else {
                    bot.send_message(
                        message.chat.id,
                        "Únicamente Queso puede utilizar este comando.",
                    )
                    .await?;
                }
            }
        }
        BotCommand::Dia => {
            if let Some(user) = message.from {
                let list = state.lock().await.data.upload_range(0);

                if list.is_empty() {
                    bot.send_message(user.id, "No hay ninguna canción subida hoy.")
                        .await?;
                } else {
                    bot.forward_messages(user.id, Recipient::Id(ChatId(Data::CHANNEL_MUSIC)), list)
                        .await?;
                }
            }
        }
        BotCommand::Semana => {
            if let Some(user) = message.from {
                let list = state.lock().await.data.upload_range(6);

                if list.is_empty() {
                    bot.send_message(user.id, "No hay ninguna canción subida esta semana.")
                        .await?;
                } else {
                    bot.forward_messages(user.id, Recipient::Id(ChatId(Data::CHANNEL_MUSIC)), list)
                        .await?;
                }
            }
        }
        BotCommand::Mes => {
            if let Some(user) = message.from {
                let list = state.lock().await.data.upload_range(29);

                if list.is_empty() {
                    bot.send_message(user.id, "No hay ninguna canción subida este mes.")
                        .await?;
                } else {
                    bot.forward_messages(user.id, Recipient::Id(ChatId(Data::CHANNEL_MUSIC)), list)
                        .await?;
                }
            }
        }
        BotCommand::LastUsuario(name) => {
            if name.is_empty() {
                bot.send_message(message.chat.id, "Tenés que dar el nombre del usuario. Ejemplo: /lastusuario@clemen_dc_bot luxreduxdelux")
                        .await?;
            } else {
                let last = &state.lock().await.last;

                if let Ok(user) = last.user_info(name).fetch().await {
                    bot.send_message(
                        message.chat.id,
                        format!(
                            r#"<a href="{}">{}</a> - {} scrobbles"#,
                            user.url, user.name, user.play_count,
                        ),
                    )
                    .parse_mode(ParseMode::Html)
                    .await?;
                } else {
                    bot.send_message(message.chat.id, "No hay ningún usuario con ese nombre.")
                        .await?;
                }
            }
        }
        BotCommand::CancionSemanal(name) => {
            if name.is_empty() {
                bot.send_message(message.chat.id, "Tenés que dar el nombre del usuario. Ejemplo: /cancionsemanal@clemen_dc_bot luxreduxdelux")
                        .await?;
            } else {
                let last = &state.lock().await.last;

                if let Ok(user) = last.weekly_chart_list(&name).fetch().await {
                    if let Some(range) = user.last()
                        && let Ok(list) = last.weekly_track_chart(name).range(range).fetch().await
                    {
                        let mut result = String::new();

                        for i in list {
                            if i.rank > 10 {
                                break;
                            }

                            result.push_str(&format!(
                                r#"{}. <a href="{}">{}</a> - {} reproducciones"#,
                                i.rank, i.url, i.name, i.playcount
                            ));
                            result.push('\n');
                        }

                        bot.send_message(message.chat.id, result)
                            .parse_mode(ParseMode::Html)
                            .await?;
                    }
                } else {
                    bot.send_message(message.chat.id, "No hay ningún usuario con ese nombre.")
                        .await?;
                }
            }
        }
        BotCommand::ArtistaSemanal(name) => {
            if name.is_empty() {
                bot.send_message(message.chat.id, "Tenés que dar el nombre del usuario. Ejemplo: /artistasemanal@clemen_dc_bot luxreduxdelux")
                        .await?;
            } else {
                let last = &state.lock().await.last;

                if let Ok(user) = last.weekly_chart_list(&name).fetch().await {
                    if let Some(range) = user.last()
                        && let Ok(list) = last.weekly_artist_chart(name).range(range).fetch().await
                    {
                        let mut result = String::new();

                        for i in list {
                            if i.rank > 10 {
                                break;
                            }

                            result.push_str(&format!(
                                r#"{}. <a href="{}">{}</a> - {} reproducciones"#,
                                i.rank, i.url, i.name, i.playcount
                            ));
                            result.push('\n');
                        }

                        bot.send_message(message.chat.id, result)
                            .parse_mode(ParseMode::Html)
                            .await?;
                    }
                } else {
                    bot.send_message(message.chat.id, "No hay ningún usuario con ese nombre.")
                        .await?;
                }
            }
        }
        BotCommand::AlbumSemanal(name) => {
            if name.is_empty() {
                bot.send_message(message.chat.id, "Tenés que dar el nombre del usuario. Ejemplo: /albumsemanal@clemen_dc_bot luxreduxdelux")
                        .await?;
            } else {
                let last = &state.lock().await.last;

                if let Ok(user) = last.weekly_chart_list(&name).fetch().await {
                    if let Some(range) = user.last()
                        && let Ok(list) = last.weekly_album_chart(name).range(range).fetch().await
                    {
                        let mut result = String::new();

                        for i in list {
                            if i.rank > 10 {
                                break;
                            }

                            result.push_str(&format!(
                                r#"{}. <a href="{}">{}</a> - {} reproducciones"#,
                                i.rank, i.url, i.name, i.playcount
                            ));
                            result.push('\n');
                        }

                        bot.send_message(message.chat.id, result)
                            .parse_mode(ParseMode::Html)
                            .await?;
                    }
                } else {
                    bot.send_message(message.chat.id, "No hay ningún usuario con ese nombre.")
                        .await?;
                }
            }
        }
        BotCommand::Vivo => {
            bot.send_message(message.chat.id, "Acá estoy.").await?;
        }
    }

    Ok(())
}

// false positive.
#[allow(clippy::collapsible_if)]
async fn handle_message(
    bot: Bot,
    message: Message,
    state: Arc<Mutex<State>>,
) -> ResponseResult<()> {
    if let teloxide::types::ChatKind::Public(_) = message.chat.kind
        && message.chat.id.0 != Data::CHANNEL_MUSIC
    {
        return Ok(());
    }

    if let Some(text) = message.text() {
        if text.contains("youtube") || text.contains("youtu.be") && !text.contains("playlist") {
            for link in YouTube::get_link_list(text) {
                let split: Vec<&str> = link.split("&").collect();
                let text = split.first().unwrap();

                bot.send_message(
                    ChatId(Data::CHANNEL_DEBUG),
                    format!("Downloading URL: \n\n{text}"),
                )
                .await?;

                for x in 0..3 {
                    let download = timeout(
                        Duration::from_secs(20),
                        state.lock().await.tube.download(text),
                    )
                    .await;

                    if let Ok(download) = download {
                        match download {
                            Ok(video) => {
                                let title = "clemen_out/audio.mp4";
                                let (artist, track, _) = YouTube::try_artist_track(&video);

                                bot.send_message(
                                    ChatId(Data::CHANNEL_DEBUG),
                                    format!(
                                        "Video download done. Sending as \"{artist} - {track:?}\"."
                                    ),
                                )
                                .await?;

                                let bot_upload =
                                    bot.send_audio(message.chat.id, InputFile::file(title));

                                let upload = if let Some(track) = track {
                                    bot_upload.title(track).performer(artist)
                                } else {
                                    bot_upload.title(artist)
                                };

                                if let Ok(Ok(bot_upload)) =
                                    timeout(Duration::from_secs(120), upload).await
                                {
                                    bot.send_message(
                                        ChatId(Data::CHANNEL_DEBUG),
                                        "Video upload done.",
                                    )
                                    .await?;

                                    if let teloxide::types::ChatKind::Public(_) = message.chat.kind
                                        && message.chat.id.0 == Data::CHANNEL_MUSIC
                                    {
                                        state
                                            .lock()
                                            .await
                                            .data
                                            .add_upload(Upload::new(bot_upload.id.0));
                                    }

                                    std::fs::remove_file(title).unwrap();

                                    break;
                                } else {
                                    bot.send_message(
                                        ChatId(Data::CHANNEL_DEBUG),
                                        format!("TG time-out error (#{x}) for URL: \n\n{text}"),
                                    )
                                    .await?;

                                    continue;
                                }
                            }
                            Err(error) => {
                                bot.send_message(
                                    ChatId(Data::CHANNEL_DEBUG),
                                    format!("YT download error (#{x}): \n\n{error:?}"),
                                )
                                .await?;

                                continue;
                            }
                        }
                    } else {
                        bot.send_message(
                            ChatId(Data::CHANNEL_DEBUG),
                            format!("YT time-out error (#{x}) for URL: \n\n{text}"),
                        )
                        .await?;

                        continue;
                    }
                }
            }
        }
    } else if message.audio().is_some() {
        if let teloxide::types::ChatKind::Public(_) = message.chat.kind
            && message.chat.id.0 == Data::CHANNEL_MUSIC
        {
            state
                .lock()
                .await
                .data
                .add_upload(Upload::new(message.id.0));
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bot = Bot::new(include_str!("../clemen.env"));

    let state = Arc::new(Mutex::new(State::new().await?));

    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<BotCommand>()
                .endpoint(handle_command),
        )
        .branch(dptree::endpoint(handle_message));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .build()
        .dispatch()
        .await;

    Ok(())
}
