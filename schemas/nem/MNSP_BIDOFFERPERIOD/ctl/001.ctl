-- ************************************************************
-- Title:	PUBLIC_DVD_MNSP_PEROFFER_202104.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MNSP_BIDOFFERPERIOD owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MNSP_PEROFFER_202104010000.CSV
APPEND INTO TABLE MNSP_BIDOFFERPERIOD
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
TRADINGDATE DATE "yyyy/mm/dd hh24:mi:ss",
OFFERDATETIME TIMESTAMP(3),
LINKID CHAR(20),
PERIODID FLOAT EXTERNAL,
MAXAVAIL FLOAT EXTERNAL,
BANDAVAIL1 FLOAT EXTERNAL,
BANDAVAIL2 FLOAT EXTERNAL,
BANDAVAIL3 FLOAT EXTERNAL,
BANDAVAIL4 FLOAT EXTERNAL,
BANDAVAIL5 FLOAT EXTERNAL,
BANDAVAIL6 FLOAT EXTERNAL,
BANDAVAIL7 FLOAT EXTERNAL,
BANDAVAIL8 FLOAT EXTERNAL,
BANDAVAIL9 FLOAT EXTERNAL,
BANDAVAIL10 FLOAT EXTERNAL,
FIXEDLOAD FLOAT EXTERNAL,
RAMPUPRATE FLOAT EXTERNAL,
PASAAVAILABILITY FLOAT EXTERNAL)
-- End of Script
