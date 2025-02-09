//! Import and export from archives in some sort of storage backend.
//!
//! ```
//! use indoc::indoc;
//! use tempfile::tempdir;
//! use dirtabase::storage::simple::storage;
//! use dirtabase::stream::{debug,archive};
//!
//! let dir = tempdir()?;
//! let store = storage(&dir)?;
//!
//! // The triad is a reference to where an archive was stored within a store
//! let triad = debug::source(archive::sink(&store))?;
//! let txt = archive::source(&store, triad, debug::sink())?;
//!
//! // We just stored our standard example archive into the store, then
//! // pulled it back out in text summary form. Neat!
//! assert_eq!(&txt, indoc! {"
//!   DIR /a/directory
//!     Foo: Bar
//!   FILE /some/dir/hello.txt
//!     Length: 17
//!     AnotherAttr: for example purposes
//! "});
//!
//! # Ok::<(), std::io::Error>(())
//! ```

use crate::archive::api::*;
use crate::archive::core::*;
use crate::storage::traits::*;
use crate::stream::core::Sink;
use std::io::{Cursor, Error, ErrorKind, Read, Result};

/// Stream files and directories into a stored Archive.
///
/// This requires already having a store. It will save files into the store as
/// you submit them. The Archive itself is serialized and saved to store at the
/// end, which is the Triad returned by .finalize().
pub fn sink<'a, S>(store: &'a S) -> ArchiveSink<'a, S>
where
    S: Storage,
{
    ArchiveSink {
        store: store,
        archive: vec![],
        format: ArchiveFormat::JSON,
        compression: Compression::Plain,
    }
}

/// Implementation of sink(&store).
pub struct ArchiveSink<'a, S>
where
    S: Storage,
{
    store: &'a S,
    archive: Archive,
    format: ArchiveFormat,
    compression: Compression,
}

impl<S> Sink for ArchiveSink<'_, S>
where
    S: Storage,
{
    type Receipt = Triad;

    fn send_dir(mut self, path: impl AsRef<Path>, attrs: Attrs) -> Result<Self> {
        self.archive.push(Entry::Dir {
            path: path.as_ref().into(),
            attrs: attrs,
        });
        Ok(self)
    }

    fn send_file(mut self, path: impl AsRef<Path>, attrs: Attrs, r: impl Read) -> Result<Self> {
        let digest = self.store.cas().write(r)?;
        self.archive.push(Entry::File {
            path: path.as_ref().into(),
            attrs: attrs,
            compression: Compression::Plain,
            digest: digest,
        });
        Ok(self)
    }

    fn finalize(self) -> Result<Triad> {
        let bytes = archive_encode(&self.archive, self.format, self.compression)?;
        let digest = self.store.cas().write(Cursor::new(bytes))?;
        // dbg!(self.archive);
        Ok(Triad(
            TriadFormat::Archive(self.format),
            self.compression,
            digest,
        ))
    }
}

/// Read an archive from a store as a series of stream events.
///
/// This requires you to have a store, but also a Triad to say which archive
/// within that store you want to read. Because of the Stream API this works
/// by driving some kind of Sink.
pub fn source<S>(store: &impl Storage, triad: Triad, mut sink: S) -> Result<S::Receipt>
where
    S: Sink,
{
    let (f, c, d) = (triad.0, triad.1, triad.2);
    let f = match f {
        TriadFormat::File => {
            return Err(Error::new(
                ErrorKind::NotFound,
                "Cannot read a file as an archive",
            ))
        }
        TriadFormat::Archive(f) => f,
    };

    let opt_reader = store.cas().read(&d)?;
    let mut r = opt_reader.ok_or(Error::new(
        ErrorKind::NotFound,
        "Source digest doesn't exist in store",
    ))?;

    let mut bytes: Vec<u8> = vec![];
    r.read_to_end(&mut bytes)?;

    let archive = archive_decode(bytes, f, c)?;
    for entry in archive {
        sink = match entry {
            Entry::Dir { path, attrs } => sink.send_dir(path, attrs)?,
            Entry::File {
                path,
                attrs,
                compression: _,
                digest,
            } => {
                let opt_reader = store.cas().read(&digest)?;
                let r = opt_reader.ok_or(Error::new(
                    ErrorKind::NotFound,
                    "Source digest doesn't exist in store",
                ))?;
                sink.send_file(path, attrs, r)?
            }
        }
    }

    sink.finalize()
}

#[cfg(test)]
mod test {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip() -> Result<()> {
        use crate::storage::simple::storage;
        use crate::stream::debug;

        let dir = tempdir()?;
        let store = storage(&dir)?;
        let arc_sink = sink(&store);
        let triad = debug::source(arc_sink)?;

        let txt = source(&store, triad, debug::sink())?;
        assert_eq!(txt, debug::source(debug::sink())?);

        Ok(())
    }
}
