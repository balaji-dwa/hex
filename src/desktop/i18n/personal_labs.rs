//! Labs › Planner's and Labs › Team sign-in's strings (this personal fork's
//! macOS labs), kept beside the main tables so those stay under the source
//! cap. The six-column rows live in [`super::labs`] and [`super::menu`].

#[rustfmt::skip]
pub(super) const FRENCH: &[(&str, &str)] = &[
    ("Team sign-in", "Connexion à l’équipe"),
    ("Sign in with your team Google account to sync Hex context later", "Connectez-vous avec le compte Google de votre équipe pour synchroniser le contexte de Hex ultérieurement"),
    ("Sign in with Google", "Se connecter avec Google"),
    ("Finish sign-in in your browser…", "Terminez la connexion dans votre navigateur…"),
    ("Signed in as {}", "Connecté en tant que {}"),
    ("Sign out", "Se déconnecter"),
    ("Checking team session…", "Vérification de la session d’équipe…"),
    ("Sign-in canceled.", "Connexion annulée."),
    ("Team sign-in is configured incorrectly.", "La connexion à l’équipe est mal configurée."),
    ("Couldn't reach team sign-in. Try again.", "Impossible de joindre le service de connexion à l’équipe. Réessayez."),
    ("Couldn't verify the browser response. Try again.", "Impossible de vérifier la réponse du navigateur. Réessayez."),
    ("Couldn't open your browser.", "Impossible d’ouvrir votre navigateur."),
    ("Sign-in timed out. Try again.", "Le délai de connexion a expiré. Réessayez."),
    ("Sign-in was declined.", "La connexion a été refusée."),
    ("Couldn't verify your team account. Try again.", "Impossible de vérifier votre compte d’équipe. Réessayez."),
    ("Your team session expired. Sign in again.", "Votre session d’équipe a expiré. Reconnectez-vous."),
    ("Couldn't access the saved team session.", "Impossible d’accéder à la session d’équipe enregistrée."),
    ("Planner", "Planificateur"),
    ("Plan today’s focus each morning at 6 AM Eastern and review goals weekly. Turns on Launch at login; open Today’s Steps from the menu bar or Option.", "Planifiez votre priorité chaque matin à 6 h, heure de l’Est, et faites le point chaque semaine. Active le lancement à la connexion ; ouvrez Étapes du jour depuis la barre des menus ou avec Option."),
    ("Today's steps", "Étapes du jour"),
];

#[rustfmt::skip]
pub(super) const PORTUGUESE: &[(&str, &str)] = &[
    ("Team sign-in", "Login da equipe"),
    ("Sign in with your team Google account to sync Hex context later", "Entre com a conta do Google da sua equipe para sincronizar o contexto do Hex mais tarde"),
    ("Sign in with Google", "Entrar com o Google"),
    ("Finish sign-in in your browser…", "Conclua o login no navegador…"),
    ("Signed in as {}", "Sessão iniciada como {}"),
    ("Sign out", "Sair"),
    ("Checking team session…", "Verificando a sessão da equipe…"),
    ("Sign-in canceled.", "Login cancelado."),
    ("Team sign-in is configured incorrectly.", "O login da equipe está configurado incorretamente."),
    ("Couldn't reach team sign-in. Try again.", "Não foi possível acessar o login da equipe. Tente novamente."),
    ("Couldn't verify the browser response. Try again.", "Não foi possível verificar a resposta do navegador. Tente novamente."),
    ("Couldn't open your browser.", "Não foi possível abrir o navegador."),
    ("Sign-in timed out. Try again.", "O tempo para fazer login acabou. Tente novamente."),
    ("Sign-in was declined.", "O login foi recusado."),
    ("Couldn't verify your team account. Try again.", "Não foi possível verificar sua conta de equipe. Tente novamente."),
    ("Your team session expired. Sign in again.", "Sua sessão de equipe expirou. Entre novamente."),
    ("Couldn't access the saved team session.", "Não foi possível acessar a sessão de equipe salva."),
    ("Planner", "Planejador"),
    ("Plan today’s focus each morning at 6 AM Eastern and review goals weekly. Turns on Launch at login; open Today’s Steps from the menu bar or Option.", "Planeje seu foco do dia todas as manhãs às 6h do horário do Leste e revise suas metas semanalmente. Ativa a abertura ao iniciar sessão; abra Passos de hoje pela barra de menus ou com Option."),
    ("Today's steps", "Passos de hoje"),
];

#[rustfmt::skip]
pub(super) const RUSSIAN: &[(&str, &str)] = &[
    ("Team sign-in", "Вход в команду"),
    ("Sign in with your team Google account to sync Hex context later", "Войдите в аккаунт Google своей команды, чтобы позже синхронизировать контекст Hex"),
    ("Sign in with Google", "Войти через Google"),
    ("Finish sign-in in your browser…", "Завершите вход в браузере…"),
    ("Signed in as {}", "Вы вошли как {}"),
    ("Sign out", "Выйти"),
    ("Checking team session…", "Проверяем сеанс команды…"),
    ("Sign-in canceled.", "Вход отменён."),
    ("Team sign-in is configured incorrectly.", "Вход в команду настроен неправильно."),
    ("Couldn't reach team sign-in. Try again.", "Не удалось связаться со службой входа в команду. Повторите попытку."),
    ("Couldn't verify the browser response. Try again.", "Не удалось проверить ответ браузера. Повторите попытку."),
    ("Couldn't open your browser.", "Не удалось открыть браузер."),
    ("Sign-in timed out. Try again.", "Время ожидания входа истекло. Повторите попытку."),
    ("Sign-in was declined.", "Во входе отказано."),
    ("Couldn't verify your team account. Try again.", "Не удалось проверить аккаунт команды. Повторите попытку."),
    ("Your team session expired. Sign in again.", "Сеанс команды истёк. Войдите снова."),
    ("Couldn't access the saved team session.", "Не удалось получить доступ к сохранённому сеансу команды."),
    ("Planner", "Планировщик"),
    ("Plan today’s focus each morning at 6 AM Eastern and review goals weekly. Turns on Launch at login; open Today’s Steps from the menu bar or Option.", "Планируйте фокус дня каждое утро в 6:00 по восточному времени США и проверяйте цели еженедельно. Включает запуск при входе; открывайте «Шаги на сегодня» из строки меню или клавишей Option."),
    ("Today's steps", "Шаги на сегодня"),
];

/// Hungarian, for the strings above.
#[rustfmt::skip]
pub(super) const HUNGARIAN: &[(&str, &str)] = &[
    ("Team sign-in", "Csapatbejelentkezés"),
    ("Sign in with your team Google account to sync Hex context later", "Jelentkezzen be a csapata Google-fiókjával, hogy később szinkronizálhassa a Hex kontextusát"),
    ("Sign in with Google", "Bejelentkezés Google-fiókkal"),
    ("Finish sign-in in your browser…", "Fejezze be a bejelentkezést a böngészőben…"),
    ("Signed in as {}", "Bejelentkezve mint {}"),
    ("Sign out", "Kijelentkezés"),
    ("Checking team session…", "Csapatmunkamenet ellenőrzése…"),
    ("Sign-in canceled.", "A bejelentkezés megszakítva."),
    ("Team sign-in is configured incorrectly.", "A csapatbejelentkezés hibásan van beállítva."),
    ("Couldn't reach team sign-in. Try again.", "A csapatbejelentkezés nem érhető el. Próbálja újra."),
    ("Couldn't verify the browser response. Try again.", "Nem sikerült ellenőrizni a böngésző válaszát. Próbálja újra."),
    ("Couldn't open your browser.", "Nem sikerült megnyitni a böngészőt."),
    ("Sign-in timed out. Try again.", "A bejelentkezés időtúllépés miatt megszakadt. Próbálja újra."),
    ("Sign-in was declined.", "A bejelentkezést elutasították."),
    ("Couldn't verify your team account. Try again.", "Nem sikerült ellenőrizni a csapatfiókját. Próbálja újra."),
    ("Your team session expired. Sign in again.", "A csapatmunkamenet lejárt. Jelentkezzen be újra."),
    ("Couldn't access the saved team session.", "Nem sikerült hozzáférni a mentett csapatmunkamenethez."),
    ("Planner", "Tervező"),
    ("Plan today’s focus each morning at 6 AM Eastern and review goals weekly. Turns on Launch at login; open Today’s Steps from the menu bar or Option.", "Tervezze meg a nap fókuszát minden reggel 6-kor (keleti parti idő szerint), és hetente tekintse át a céljait. Bekapcsolja az indítást bejelentkezéskor; a Mai lépéseket a menüsorból vagy az Option billentyűvel nyithatja meg."),
    ("Today's steps", "Mai lépések"),
];
