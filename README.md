# Photo Renamer

Photo and movie renaming utility. Trying to capture the renaming functionality of Rapid Photo Downloader, but without the downloading and all the other bits:

- Only copy files once over multiple runs.
- Use EXIF data for date determination, with fallbacks for filename and modified date.
- Attempt to use EXIF data for raw file date determination if possible.

## Usage

Once compiled, run `renamer` in a folder somewhere. It will create an empty config file (`renamer.toml`) with some vaguely sensible defaults in it. Edit those to provide:

- A list of input dirs
- Output dirs for raw and non-raw files
- Any exclusion strings you might want to use to ignore files

Then, re-run `renamer`. It will now copy all picture, RAW, and movie files from the input folder to the output folder with a date-time filename.

## Changes Welcome!
As is usually the case with these little CLIs I put together, there's not a lot in the way of "proper" error handling. There's also not many configuration options for things that have been hard-coded for my use. There may well be panics. And I know it's not very unicode savvy. If you'd like to change any of this, feel free to submit a pull request!

## License

This project is licensed under MIT license (LICENSE-MIT or https://opensource.org/licenses/MIT)
