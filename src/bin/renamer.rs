use anyhow::{anyhow, Error};
use argh::FromArgs;
use chrono::Local;
use exif::{In, Reader, Tag};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use log::{info, warn};
use regex::Regex;
use rusqlite::{Connection, Result};
use simplelog::{Config, LevelFilter, SimpleLogger};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::fs::File;
use std::io::Write as IoWrite;
use std::ops::Add;
use std::path::{Path, PathBuf, MAIN_SEPARATOR_STR};
use std::process::exit;
use walkdir::{DirEntry, WalkDir};

use photo_renamer::config::RenamerConfig;

pub const SUPPORTED_EXIF_EXTENSIONS: [&str; 3] = ["jpg", "tiff", "jpeg"];
pub const SUPPORTED_RAW_EXTENSIONS: [&str; 3] = ["dng", "rw2", "raw"];
pub const SUPPORTED_MOVIE_EXTENSIONS: [&str; 4] = ["mp4", "avi", "mpg", "mov"];

#[derive(FromArgs, PartialEq, Debug)]
/// Processes a collection of photos and videos, copying them to an output folder with a standardised
/// naming format.
struct RenamerArgs {
    #[argh(option, short = 'd', default = "\"renamer.db\".to_string()")]
    /// name of the db file used to store file copy history
    db_name: String,
    #[argh(switch, short = 't')]
    /// run in test mode, logging what would have been done in a normal run instead of performing the action
    test_mode: bool,

    #[argh(subcommand)]
    sub_command: SubCommandEnum,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommandEnum {
    Rename(RenameSubCommand),
    Rebase(RebaseSubCommand),
}

#[derive(FromArgs, PartialEq, Debug)]
/// rename and copy files as per the config file
#[argh(subcommand, name = "rename")]
struct RenameSubCommand {}

#[derive(FromArgs, PartialEq, Debug)]
/// update the source file locations in the db to allow for file-system changes
#[argh(subcommand, name = "rebase")]
struct RebaseSubCommand {
    #[argh(positional)]
    /// the original root directory of files in the file copy history to be changed
    original_file_root: String,

    #[argh(positional)]
    /// the new root directory to use in the file copy history
    destination_file_root: String,
}

/// Return a valid database connection to a local SQLlite DB, with the name specified by the
/// arguments to the CLI.
fn get_db(args: &RenamerArgs) -> Result<Connection, Error> {
    let db_path = Path::new(&args.db_name);

    let exists = db_path.exists();

    let db_connection = Connection::open(db_path)?;

    if !exists {
        db_connection.execute("CREATE TABLE files (filename TEXT, checksum TEXT)", ())?;
        db_connection.execute("CREATE UNIQUE INDEX unique_paths ON files (filename)", ())?;
    }

    Ok(db_connection)
}

fn _is_picture(file: &PathBuf) -> bool {
    if let Some(extension_str) = file.extension() {
        let extension_str = extension_str.to_str().unwrap().to_lowercase();

        return SUPPORTED_EXIF_EXTENSIONS.contains(&extension_str.as_str());
    }

    false
}

fn _is_raw(file: &PathBuf) -> bool {
    if let Some(extension_str) = file.extension() {
        let extension_str = extension_str.to_str().unwrap().to_lowercase();

        return SUPPORTED_RAW_EXTENSIONS.contains(&extension_str.as_str());
    }

    false
}

fn _is_movie(file: &PathBuf) -> bool {
    if let Some(extension_str) = file.extension() {
        let extension_str = extension_str.to_str().unwrap().to_lowercase();

        return SUPPORTED_MOVIE_EXTENSIONS.contains(&extension_str.as_str());
    }

    false
}

fn file_in_scope(file: &PathBuf) -> bool {
    _is_picture(&file) || _is_raw(&file) || _is_movie(&file)
}

fn _is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

/// Return all filenames that will be considered for processing. This means we will exclude any filenames
/// matching any exclusions from the config
fn get_all_filenames_in_scope(
    config: &RenamerConfig,
) -> Result<HashMap<String, Vec<PathBuf>>, Error> {
    info!("Determining in-scope filenames");

    let mut filenames: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for root_path_string in &config.root_paths {
        for entry in WalkDir::new(
            Path::new(root_path_string)
                .canonicalize()
                .expect("Unable to determine canonical root path"),
        )
        .into_iter()
        .filter_entry(|entry| !_is_hidden(entry))
        {
            let entry_pathbuf = entry?.into_path();

            if entry_pathbuf.is_dir() {
                continue;
            }

            let entry_path = &entry_pathbuf
                .to_str()
                .ok_or("Unable to convert path to string")
                .unwrap();

            if !file_in_scope(&entry_pathbuf) {
                continue;
            }

            let mut exclusion_found = false;

            for component in &config.exclusions {
                if entry_path.contains(component) {
                    exclusion_found = true;
                    break;
                }
            }

            if !exclusion_found {
                filenames
                    .entry(
                        entry_pathbuf
                            .file_stem()
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_string(),
                    )
                    .or_insert(vec![])
                    .push(entry_pathbuf);
            }
        }
    }

    info!("Found {} unique file stems", filenames.len());

    Ok(filenames)
}

/// Helper function to turn a filename into a SQL-safe string format.
fn get_sql_safe_filename(file: &PathBuf) -> Result<String, Error> {
    Ok(file.to_str().unwrap().replace("\\", "/").to_string())
}

/// Take a given file and target date, and copy the file into the output folder with the new filename.
fn copy_file_and_mark_as_processed(
    source_file: &PathBuf,
    output_date: &chrono::NaiveDateTime,
    renamer_config: &RenamerConfig,
    renamer_args: &RenamerArgs,
    db_connection: &Connection,
) -> Result<(), Error> {
    let mut insert_statement = db_connection.prepare("INSERT INTO files VALUES (?, ?)")?;

    let has_mp_tag = source_file
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_lowercase()
        .split('.')
        .collect::<Vec<&str>>()
        .contains(&&"mp");
    let is_mvimg = source_file
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_lowercase()
        .starts_with(&"mvimg");

    for counter in 0..99 {
        let mut new_path = if _is_raw(source_file) {
            PathBuf::from(&renamer_config.raw_output_path)
        } else {
            PathBuf::from(&renamer_config.output_path)
        };

        // Create the output directory if not present
        if !new_path.exists() {
            fs::create_dir_all(&new_path)?;
        }

        let mut filename_components: Vec<String> = vec![];

        // New filename starts with the datetime
        filename_components.push(output_date.format("%Y%m%d_%H%M%S").to_string());

        // If not the first attempt, we must have found a duplicate filename, so bump up the counter to try again with a different name
        if counter != 0 {
            filename_components.push(counter.to_string());
        }

        if has_mp_tag || is_mvimg {
            filename_components.push("mp".to_string());
        }

        filename_components.push(
            source_file
                .extension()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
        );

        new_path.push(filename_components.join("."));

        if new_path.exists() {
            // We found a matching entry, try again, which will bump up the counter
            continue;
        }

        let final_path = new_path.to_str().unwrap().to_lowercase();

        if renamer_args.test_mode {
            info!(
                "Would have copied {} to {}",
                source_file.to_str().unwrap(),
                final_path
            );
            return Ok(());
        }

        fs::copy(source_file, final_path)?;

        let sql_safe_filename = get_sql_safe_filename(source_file)?;

        insert_statement.insert(rusqlite::params![&sql_safe_filename, &sql_safe_filename])?;

        break;
    }

    Ok(())
}

/// Extract, where possible, a datetime from a file's EXIF data.
fn extract_timestamp_from_exif(filename: &PathBuf) -> Result<chrono::NaiveDateTime, Error> {
    let input_file = File::open(&filename)?;

    // First start by trying to get hold of the exif data
    let exif_data = Reader::new().read_from_container(&mut std::io::BufReader::new(&input_file))?;

    let photo_datetime_field = match exif_data.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
        None => {
            return Err(anyhow!(
                "DateTimeOriginal field not available for {:?}",
                filename
            ));
        }
        Some(field) => field,
    };

    let disp = photo_datetime_field
        .value
        .display_as(Tag::DateTimeOriginal)
        .to_string();

    Ok(chrono::NaiveDateTime::parse_from_str(
        &disp,
        "%Y-%m-%d %H:%M:%S",
    )?)
}

/// Extract, where possible, a datetime from a file's name.
fn extract_datetime_from_filename(file: &PathBuf) -> Option<chrono::NaiveDateTime> {
    let filename = file.file_stem()?.to_str()?;

    let filename_regex = Regex::new(r"(\d{8})[-_]?(\d{6})").ok()?;

    if let Some(captures) = filename_regex.captures(&filename) {
        if captures.len() == 1 {
            let first_capture = captures.get(1)?;
            let second_capture = captures.get(2)?;

            if let Ok(file_date) = chrono::NaiveDateTime::parse_from_str(
                format!("{}{}", first_capture.as_str(), second_capture.as_str()).as_str(),
                "%Y%m%d%H%M%S",
            ) {
                return Some(file_date);
            };
        };
    };

    None
}

/// Extract, where possible, a datetime from a file's metadata, specifically, the file's modified time.
fn extract_datetime_from_file_metadata(file: &PathBuf) -> Option<chrono::NaiveDateTime> {
    if let Some(file_metadata) = fs::metadata(file).ok() {
        if let Ok(file_metadata_modified) = file_metadata.modified() {
            return Some(chrono::DateTime::<Local>::from(file_metadata_modified).naive_local());
        }
    }

    None
}

fn has_file_been_processed(db_connection: &Connection, path: &PathBuf) -> bool {
    let mut check_statement = db_connection
        .prepare("SELECT * FROM files WHERE filename = ?")
        .unwrap();

    check_statement
        .exists(rusqlite::params![&get_sql_safe_filename(path).unwrap()])
        .unwrap()
}

/// Process all filenames, copying them if not already copied and if it is possible to determine a valid
/// date to use for output filename formatting.
fn process_files(
    db_connection: &Connection,
    filenames: &HashMap<String, Vec<PathBuf>>,
    renamer_config: &RenamerConfig,
    renamer_args: &RenamerArgs,
) -> Result<(), Error> {
    info!("Beginning media rename operation...");

    let mut successful_file_copy_count = 0;
    let mut processed_file_count: u64 = 0;
    let pb = ProgressBar::new(filenames.len() as u64);

    // Shamelessly copied from https://github.com/console-rs/indicatif/blob/HEAD/examples/download.rs
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})",
        )?
        .with_key("eta", |state: &ProgressState, w: &mut dyn FmtWrite| {
            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
        })
        .progress_chars("#>-"),
    );

    let mut errors: Vec<String> = vec![];

    for (_, paths) in filenames {
        // Before we begin, let's update the status bar
        processed_file_count += 1;
        pb.set_position(processed_file_count);

        // Firstly, check whether all paths in the paths vec have been processed already. If so, we can
        // move on now.
        if paths
            .iter()
            .map(|path| has_file_been_processed(db_connection, path))
            .all(|result| result == true)
        {
            continue;
        }

        // Try to determine a unique datetime for the files with the same prefix. We may be mixing
        // raws with jpgs, and getting raw file info is harder than it seems apparently, so if we can get a single unique
        // datetime from one or more jpgs, we can assume they apply to any raws too.
        let potential_dates = paths
            .into_iter()
            .map(|path| extract_timestamp_from_exif(path))
            .filter(|result| result.is_ok())
            .map(|result| result.unwrap())
            .collect::<HashSet<chrono::NaiveDateTime>>();

        for path in paths {
            if !file_in_scope(path) {
                continue;
            }

            // We have a file we can investigate. Check whether we've seen it before. If so, we'll skip
            if has_file_been_processed(db_connection, path) {
                continue;
            }

            if potential_dates.len() == 1 {
                copy_file_and_mark_as_processed(
                    path,
                    &potential_dates.iter().next().unwrap(),
                    renamer_config,
                    renamer_args,
                    db_connection,
                )?;

                successful_file_copy_count += 1;

                continue;
            }

            // No unique data for the files could be determined, so we'll need to get a bit funky here. Let's start
            // by trying to parse the date and time of the file from the filename
            if let Some(filename_datetime) = extract_datetime_from_filename(path) {
                copy_file_and_mark_as_processed(
                    &path,
                    &filename_datetime,
                    renamer_config,
                    renamer_args,
                    db_connection,
                )?;

                successful_file_copy_count += 1;

                continue;
            }

            // Apparently, that didn't work either. Next step - file timestamps time.
            if let Some(filename_datetime) = extract_datetime_from_file_metadata(path) {
                copy_file_and_mark_as_processed(
                    &path,
                    &filename_datetime,
                    renamer_config,
                    renamer_args,
                    db_connection,
                )?;

                successful_file_copy_count += 1;

                continue;
            }

            // At this stage, you're just out of luck
            let filename_string = String::from(path.to_str().unwrap());

            errors.push(format!(
                "Unable to determine valid datetime for {}",
                filename_string
            ));
        }
    }

    // Finally, write out the errors to disk.
    if !errors.is_empty() {
        if let Ok(mut config_file) =
            File::create(Local::now().format("%Y%m%d_%H%M%S_errors.log").to_string())
        {
            config_file.write(&errors.join("\n").as_bytes())?;
        }

        warn!("Errors found when copying {} files", errors.len());

        return Err(anyhow!("{} errors found when renaming files", errors.len()));
    }

    if successful_file_copy_count > 0 {
        info!("Copied {} files", successful_file_copy_count);
    }

    Ok(())
}

fn process_rebase(args: &RenamerArgs, rebase_args: &RebaseSubCommand) -> Result<(), Error> {
    let mut source_root = get_sql_safe_filename(&PathBuf::from(&rebase_args.original_file_root))?;
    let mut dest_root = get_sql_safe_filename(&PathBuf::from(&rebase_args.destination_file_root))?;

    if !source_root.ends_with(MAIN_SEPARATOR_STR) {
        source_root = source_root.add(MAIN_SEPARATOR_STR);
    }

    if !dest_root.ends_with(MAIN_SEPARATOR_STR) {
        dest_root = dest_root.add(MAIN_SEPARATOR_STR);
    }

    let db_connection = get_db(&args)?;

    if args.test_mode {
        let mut select_statement =
            db_connection.prepare("SELECT COUNT(*) FROM files WHERE filename like ?")?;

        let updated_rows =
            select_statement.query_row(rusqlite::params![format!("{}%", &source_root)], |row| row.get::<_, i32>(0))?;

        info!(
            "Would have updated {} path roots from {} to {}",
            updated_rows, &source_root, &dest_root
        );
    } else {
        let mut update_statement = db_connection
            .prepare("UPDATE files SET filename = replace(filename, ?, ?) WHERE filename like ?")?;

        let updated_rows = update_statement.execute(rusqlite::params![
            &source_root,
            &dest_root,
            format!("{}%", &source_root)
        ])?;

        info!(
            "Updated path roots from {} to {} - {} rows affected",
            &source_root, &dest_root, updated_rows
        );
    }

    info!("Rebase complete");

    Ok(())
}

fn process_rename(args: &RenamerArgs, _: &RenameSubCommand) -> Result<(), Error> {
    // Try and read config file into object. If none was found, this will be None, so we can finish up
    let config = match RenamerConfig::read_or_create()? {
        None => {
            return Ok(());
        }
        Some(conf_object) => conf_object,
    };

    let db_connection = get_db(&args)?;
    let filenames = get_all_filenames_in_scope(&config)?;

    process_files(&db_connection, &filenames, &config, &args)?;

    info!("Rename complete");

    Ok(())
}

fn run() -> Result<(), Error> {
    let args: RenamerArgs = argh::from_env();

    match args.sub_command {
        SubCommandEnum::Rename(ref rename_args) => process_rename(&args, &rename_args),
        SubCommandEnum::Rebase(ref rebase_args) => process_rebase(&args, &rebase_args),
    }?;

    Ok(())
}

fn main() -> Result<(), Error> {
    SimpleLogger::init(LevelFilter::Info, Config::default())?;

    run()?;

    exit(0)
}
