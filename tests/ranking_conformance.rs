use minisearch_wasm::{FuzzySetting, MiniSearch, MiniSearchOptions, SearchOptions, TokenizerMode};
use serde_json::{json, Value};

fn index(fields: &[&str], store_field: &str, documents: Vec<Value>) -> MiniSearch {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        id_field: "id".to_owned(),
        store_fields: vec![store_field.to_owned()],
        tokenizer: TokenizerMode::Default,
        search_options: SearchOptions::default(),
        auto_suggest_options: None,
        auto_vacuum: None,
    });
    search.add_all(documents).unwrap();
    search
}

fn stored_values(
    search: &MiniSearch,
    query: &str,
    options: SearchOptions,
    field: &str,
) -> Vec<String> {
    search
        .search(query, options)
        .into_iter()
        .map(|result| result.stored_fields[field].as_str().unwrap().to_owned())
        .collect()
}

fn fuzzy_prefix() -> SearchOptions {
    SearchOptions {
        fuzzy: Some(FuzzySetting::Distance(1.0)),
        prefix: true.into(),
        ..SearchOptions::default()
    }
}

fn movie_index() -> MiniSearch {
    index(
        &["title", "description"],
        "title",
        vec![
            json!({
                "id": "tt1487931",
                "title": "Khumba",
                "description": "When half-striped zebra Khumba is blamed for the lack of rain by the rest of his insular, superstitious herd, he embarks on a daring quest to earn his stripes. In his search for the legendary waterhole in which the first zebras got their stripes, Khumba meets a quirky range of characters and teams up with an unlikely duo: overprotective wildebeest Mama V and Bradley, a self-obsessed, flamboyant ostrich. But before he can reunite with his herd, Khumba must confront Phango, a sadistic leopard who controls the waterholes and terrorizes all the animals in the Great Karoo. It's not all black-and-white in this colorful adventure with a difference."
            }),
            json!({
                "id": "tt8737608",
                "title": "Rams",
                "description": "A feud between two sheep farmers."
            }),
            json!({
                "id": "tt0983983",
                "title": "Shaun the Sheep",
                "description": "Shaun is a cheeky and mischievous sheep at Mossy Bottom farm who's the leader of the flock and always plays slapstick jokes, pranks and causes trouble especially on Farmer X and his grumpy guide dog, Bitzer."
            }),
            json!({
                "id": "tt5174284",
                "title": "Shaun the Sheep: The Farmer's Llamas",
                "description": "At the annual County Fair, three peculiar llamas catch the eye of Shaun, who tricks the unsuspecting Farmer into buying them. At first, it's all fun and games at Mossy Bottom Farm until the trio of unruly animals shows their true colours, wreaking havoc before everyone's eyes. Now, it's up to Bitzer and Shaun to come up with a winning strategy, if they want to reclaim the farm. Can they rid the once-peaceful ranch of the troublemakers?"
            }),
            json!({
                "id": "tt0102926",
                "title": "The Silence of the Lambs",
                "description": "F.B.I. trainee Clarice Starling (Jodie Foster) works hard to advance her career, while trying to hide or put behind her West Virginia roots, of which if some knew, would automatically classify her as being backward or white trash. After graduation, she aspires to work in the agency's Behavioral Science Unit under the leadership of Jack Crawford (Scott Glenn). While she is still a trainee, Crawford asks her to question Dr. Hannibal Lecter (Sir Anthony Hopkins), a psychiatrist imprisoned, thus far, for eight years in maximum security isolation for being a serial killer who cannibalized his victims. Clarice is able to figure out the assignment is to pick Lecter's brains to help them solve another serial murder case, that of someone coined by the media as \"Buffalo Bill\" (Ted Levine), who has so far killed five victims, all located in the eastern U.S., all young women, who are slightly overweight (especially around the hips), all who were drowned in natural bodies of water, and all who were stripped of large swaths of skin. She also figures that Crawford chose her, as a woman, to be able to trigger some emotional response from Lecter. After speaking to Lecter for the first time, she realizes that everything with him will be a psychological game, with her often having to read between the very cryptic lines he provides. She has to decide how much she will play along, as his request in return for talking to him is to expose herself emotionally to him. The case takes a more dire turn when a sixth victim is discovered, this one from who they are able to retrieve a key piece of evidence, if Lecter is being forthright as to its meaning. A potential seventh victim is high profile Catherine Martin (Brooke Smith), the daughter of Senator Ruth Martin (Diane Baker), which places greater scrutiny on the case as they search for a hopefully still alive Catherine. Who may factor into what happens is Dr. Frederick Chilton (Anthony Heald), the warden at the prison, an opportunist who sees the higher profile with Catherine, meaning a higher profile for himself if he can insert himself successfully into the proceedings."
            }),
            json!({
                "id": "tt0395479",
                "title": "Boundin'",
                "description": "In the not too distant past, a lamb lives in the desert plateau just below the snow line. He is proud of how bright and shiny his coat of wool is, so much so that it makes him want to dance, which in turn makes all the other creatures around him also want to dance. His life changes when one spring day he is captured, his wool shorn, and thrown back out onto the plateau all naked and pink. But a bounding jackalope who wanders by makes the lamb look at life a little differently in seeing that there is always something exciting in life to bound about."
            }),
            json!({
                "id": "tt9812474",
                "title": "Lamb",
                "description": "Haunted by the indelible mark of loss and silent grief, sad-eyed María and her taciturn husband, Ingvar, seek solace in back-breaking work and the demanding schedule at their sheep farm in the remote, harsh, wind-swept landscapes of mountainous Iceland. Then, with their relationship hanging on by a thread, something unexplainable happens, and just like that, happiness blesses the couple's grim household once more. Now, as a painful ending gives birth to a new beginning, Ingvar's troubled brother, Pétur, arrives at the farmhouse, threatening María and Ingvar's delicate, newfound bliss. But, nature's gifts demand sacrifice. How far are ecstatic María and Ingvar willing to go in the name of love?"
            }),
            json!({
                "id": "tt0306646",
                "title": "Ringing Bell",
                "description": "A baby lamb named Chirin is living an idyllic life on a farm with many other sheep. Chirin is very adventurous and tends to get lost, so he wears a bell around his neck so that his mother can always find him. His mother warns Chirin that he must never venture beyond the fence surrounding the farm, because a huge black wolf lives in the mountains and loves to eat sheep. Chirin is too young and naive to take the advice to heart, until one night the wolf enters the barn and is prepared to kill Chirin, but at the last moment the lamb's mother throws herself in the way and is killed instead. The wolf leaves, and Chirin is horrified to see his mother's body. Unable to understand why his mother was killed, he becomes very angry and swears that he will go into the mountains and kill the wolf."
            }),
            json!({
                "id": "tt1212022",
                "title": "The Lion of Judah",
                "description": "Follow the adventures of a bold lamb (Judah) and his stable friends as they try to avoid the sacrificial alter the week preceding the crucifixion of Christ. It is a heart-warming account of the Easter story as seen through the eyes of a lovable pig (Horace), a faint-hearted horse (Monty), a pedantic rat (Slink), a rambling rooster (Drake), a motherly cow (Esmay) and a downtrodden donkey (Jack). This magnificent period piece with its epic sets is a roller coaster ride of emotions. Enveloped in humor, this quest follows the animals from the stable in Bethlehem to the great temple in Jerusalem and onto the hillside of Calvary as these unlikely heroes try to save their friend. The journey weaves seamlessly through the biblical accounts of Palm Sunday, Jesus turning the tables in the temple, Peter's denial and with a tense, heart-wrenching climax, depicts the crucifixion and resurrection with gentleness and breathtaking beauty. For Judah, the lamb with the heart of a lion, it is a story of courage and faith. For Jack, the disappointed donkey, it becomes a pivotal voyage of hope. For Horace, the, well the dirty pig, and Drake the ignorant rooster, it is an opportunity to do something inappropriate and get into trouble."
            }),
        ],
    )
}

#[test]
fn movie_ranking_matches_reference_fixtures() {
    let search = movie_index();

    assert_eq!(
        stored_values(&search, "lamb", fuzzy_prefix(), "title"),
        vec![
            "Lamb",
            "Boundin'",
            "Ringing Bell",
            "The Lion of Judah",
            "The Silence of the Lambs",
        ]
    );
    assert_eq!(
        stored_values(&search, "sheep", fuzzy_prefix(), "title"),
        vec![
            "Shaun the Sheep",
            "Rams",
            "Shaun the Sheep: The Farmer's Llamas",
            "Ringing Bell",
            "Lamb",
        ]
    );

    for (query, expected) in [
        ("shaun the sheep", "Shaun the Sheep"),
        ("chirin the sheep", "Ringing Bell"),
        ("judah the sheep", "The Lion of Judah"),
    ] {
        assert_eq!(
            stored_values(&search, query, SearchOptions::default(), "title")[0],
            expected,
            "query={query}"
        );
        assert_eq!(
            stored_values(&search, query, fuzzy_prefix(), "title")[0],
            expected,
            "fuzzy query={query}"
        );
    }

    assert_eq!(
        stored_values(
            &search,
            "bounding sheep",
            SearchOptions {
                fuzzy: Some(FuzzySetting::Distance(1.0)),
                ..SearchOptions::default()
            },
            "title",
        )[0],
        "Boundin'"
    );
}

#[test]
fn song_ranking_matches_reference_fixtures() {
    let search = index(
        &["song", "artist"],
        "song",
        vec![
            json!({ "id": "1", "song": "Killer Queen", "artist": "Queen" }),
            json!({ "id": "2", "song": "The Witch Queen Of New Orleans", "artist": "Redbone" }),
            json!({ "id": "3", "song": "Waterloo", "artist": "Abba" }),
            json!({ "id": "4", "song": "Take A Chance On Me", "artist": "Abba" }),
            json!({ "id": "5", "song": "Help", "artist": "The Beatles" }),
            json!({ "id": "6", "song": "Yellow Submarine", "artist": "The Beatles" }),
            json!({ "id": "7", "song": "Dancing Queen", "artist": "Abba" }),
            json!({ "id": "8", "song": "Bohemian Rhapsody", "artist": "Queen" }),
        ],
    );

    assert_eq!(
        stored_values(&search, "witch queen", fuzzy_prefix(), "song"),
        vec![
            "The Witch Queen Of New Orleans",
            "Killer Queen",
            "Bohemian Rhapsody",
            "Dancing Queen",
        ]
    );
    assert_eq!(
        stored_values(&search, "queen", fuzzy_prefix(), "song")[0],
        "Killer Queen"
    );
}
