use std::fs;
use std::io;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};

use tokio::{sync::mpsc, task::JoinHandle};

const COMPILE_COMMANDS_JSON_FILE_NAME: &str = "compile_commands.json";

fn spawn_compile_commands_search<P>(
    path: P,
    results_channel: mpsc::Sender<PathBuf>,
) -> JoinHandle<()>
where
    P: AsRef<Path> + Send + 'static,
{
    tokio::spawn(async move {
        find_compile_commands_files(path, results_channel)
            .await
            .unwrap();
    })
}

async fn find_compile_commands_files<P>(
    path: P,
    results_channel: mpsc::Sender<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: AsRef<Path>,
{
    let mut dir_contents = tokio::fs::read_dir(path).await?;
    while let Some(entry) = dir_contents.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            // spawn a new search in subdir
            spawn_compile_commands_search(entry.path(), results_channel.clone());
        } else if entry.file_name() == COMPILE_COMMANDS_JSON_FILE_NAME {
            // compile_commands.json file found -> send it over the channel
            results_channel.send(entry.path()).await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // skip the first arg (name of the binary)
    let args: Vec<String> = std::env::args().skip(1).collect();

    // create channel to pass the compile_command.json paths from search tasks back to joining
    let (tx, mut rx) = mpsc::channel(32);
    if args.is_empty() {
        // default to current directory
        spawn_compile_commands_search(std::env::current_dir()?, tx.clone());
    } else {
        // search in all directories provided as arguments
        for path in args.into_iter() {
            spawn_compile_commands_search(path, tx.clone());
        }
    }

    // all spawn calls have a clone so let's drop the last instance so the rx.recv finishes when all tasks drop their tx
    drop(tx);

    // open output file for writing
    let mut output = io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(COMPILE_COMMANDS_JSON_FILE_NAME)?,
    );

    // start json list
    output.write_all(b"[")?;
    let mut has_contents = false;
    while let Some(path) = rx.recv().await {
        let mut input = io::BufReader::new(fs::File::open(path)?);
        let mut buffer = Vec::new();

        // advance until the list start
        input.read_until(b'[', &mut buffer)?;
        // discard what we have read so far
        buffer.clear();

        // read the rest of the file into the buffer
        input.read_to_end(&mut buffer)?;

        // drop from the end of the buffer until we find list end
        while !buffer.is_empty() && buffer.last() != Some(&b']') {
            buffer.pop();
        }

        // drop the list end character
        if buffer.last() == Some(&b']') {
            buffer.pop();
        }

        // write the buffer to the output file
        if !buffer.is_empty() {
            // write delimiter if there's already any contents written to the file
            if has_contents {
                output.write_all(b",")?;
            } else {
                has_contents = true;
            }

            output.write_all(&buffer)?;
        }
    }

    // end json list
    output.write_all(b"]")?;

    // flush before dropping the writer
    output.flush()?;

    Ok(())
}
