-- ************************************************************
-- Title:	PUBLIC_DVD_DISPATCH_MNSPBIDTRK_202104.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DISPATCH_MNSPBIDTRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_DISPATCH_MNSPBIDTRK_202104010000.CSV
APPEND INTO TABLE DISPATCH_MNSPBIDTRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
RUNNO FLOAT EXTERNAL,
PARTICIPANTID CHAR(10),
LINKID CHAR(10),
OFFERSETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
OFFEREFFECTIVEDATE TIMESTAMP(3),
OFFERVERSIONNO FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
